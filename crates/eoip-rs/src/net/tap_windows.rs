//! Windows TAP device via tap-windows6 (OpenVPN TAP driver).
//!
//! Opens `\\.\Global\{GUID}.tap` and uses overlapped ReadFile/WriteFile
//! for async packet I/O. The TAP adapter must be pre-created (via OpenVPN
//! installer or tapctl.exe).

#![cfg(target_os = "windows")]

use std::io;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::*;
use windows_sys::Win32::System::Threading::CreateEventW;
use windows_sys::Win32::System::Registry::HKEY;

/// TAP-Windows IOCTL to set media status (connected/disconnected).
/// CTL_CODE(FILE_DEVICE_UNKNOWN=0x22, function=6, METHOD_BUFFERED=0, FILE_ANY_ACCESS=0)
const TAP_WIN_IOCTL_SET_MEDIA_STATUS: u32 = 0x00220018;

/// Windows TAP device handle.
pub struct WinTapDevice {
    handle: HANDLE,
}

// SAFETY: The HANDLE is used only via ReadFile/WriteFile which are thread-safe.
unsafe impl Send for WinTapDevice {}
unsafe impl Sync for WinTapDevice {}

impl WinTapDevice {
    /// Open an existing TAP adapter by its GUID (e.g., `{0637F4AD-...}`).
    pub fn open(guid: &str) -> io::Result<Self> {
        let path = format!("\\\\.\\Global\\{}.tap\0", guid);
        let path_wide: Vec<u16> = path.encode_utf16().collect();

        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_SYSTEM | FILE_FLAG_OVERLAPPED,
                0 as HANDLE,
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let dev = Self { handle };

        // Set media status to connected with retry.
        // The first call may fail or not take effect immediately
        // on overlapped handles, so we retry after a short delay.
        for attempt in 0..3 {
            match dev.set_media_status(true) {
                Ok(()) => break,
                Err(e) if attempt < 2 => {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    eprintln!("TAP set_media_status retry {}: {}", attempt + 1, e);
                }
                Err(e) => return Err(e),
            }
        }

        Ok(dev)
    }

    /// Set the TAP adapter media status (connected/disconnected).
    ///
    /// Public so the daemon can re-call it after configuring the adapter.
    pub fn set_media_status(&self, connected: bool) -> io::Result<()> {
        let status: u32 = if connected { 1 } else { 0 };
        let mut bytes_returned: u32 = 0;
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = unsafe { CreateEventW(std::ptr::null(), TRUE as _, FALSE as _, std::ptr::null()) };

        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                TAP_WIN_IOCTL_SET_MEDIA_STATUS,
                &status as *const u32 as *const _,
                std::mem::size_of::<u32>() as u32,
                std::ptr::null_mut(),
                0,
                &mut bytes_returned,
                &mut overlapped,
            )
        };

        let result = if ok != 0 {
            Ok(())
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                unsafe { GetOverlappedResult(self.handle, &overlapped, &mut bytes_returned, TRUE as _) };
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(err as i32))
            }
        };

        if !overlapped.hEvent.is_null() {
            unsafe { CloseHandle(overlapped.hEvent) };
        }
        result
    }

    /// Read an Ethernet frame from the TAP device (blocking via overlapped wait).
    pub fn read_blocking(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut bytes_read: u32 = 0;
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = unsafe { CreateEventW(std::ptr::null(), TRUE as _, FALSE as _, std::ptr::null()) };
        if overlapped.hEvent.is_null() {
            return Err(io::Error::last_os_error());
        }

        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut bytes_read,
                &mut overlapped,
            )
        };

        let result = if ok != 0 {
            Ok(bytes_read as usize)
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                let wait_ok = unsafe {
                    GetOverlappedResult(self.handle, &overlapped, &mut bytes_read, TRUE as _)
                };
                if wait_ok != 0 {
                    Ok(bytes_read as usize)
                } else {
                    Err(io::Error::last_os_error())
                }
            } else {
                Err(io::Error::from_raw_os_error(err as i32))
            }
        };

        unsafe { CloseHandle(overlapped.hEvent) };
        result
    }

    /// Write an Ethernet frame to the TAP device (blocking via overlapped wait).
    pub fn write_blocking(&self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes_written: u32 = 0;
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = unsafe { CreateEventW(std::ptr::null(), TRUE as _, FALSE as _, std::ptr::null()) };
        if overlapped.hEvent.is_null() {
            return Err(io::Error::last_os_error());
        }

        let ok = unsafe {
            WriteFile(
                self.handle,
                buf.as_ptr().cast(),
                buf.len() as u32,
                &mut bytes_written,
                &mut overlapped,
            )
        };

        let result = if ok != 0 {
            Ok(bytes_written as usize)
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                let wait_ok = unsafe {
                    GetOverlappedResult(self.handle, &overlapped, &mut bytes_written, TRUE as _)
                };
                if wait_ok != 0 {
                    Ok(bytes_written as usize)
                } else {
                    Err(io::Error::last_os_error())
                }
            } else {
                Err(io::Error::from_raw_os_error(err as i32))
            }
        };

        unsafe { CloseHandle(overlapped.hEvent) };
        result
    }
}

    /// Set the MTU on this TAP adapter via `netsh`.
    ///
    /// Requires the adapter to have an assigned name (connection name in
    /// Windows Network Connections). The `adapter_name` is the human-readable
    /// name (e.g. "Local Area Connection 2"), not the GUID.
    pub fn set_mtu(adapter_name: &str, mtu: u16) -> io::Result<()> {
        let output = std::process::Command::new("netsh")
            .args([
                "interface", "ipv4", "set", "subinterface",
                adapter_name,
                &format!("mtu={mtu}"),
                "store=persistent",
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("netsh set mtu failed: {stderr}"),
            ));
        }
        Ok(())
    }
}

impl Drop for WinTapDevice {
    fn drop(&mut self) {
        let _ = self.set_media_status(false);
        unsafe { CloseHandle(self.handle) };
    }
}

/// Read a registry string value as UTF-16 → String.
unsafe fn read_reg_string(hkey: HKEY, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::*;

    let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 256];
    let mut buf_size = (buf.len() * 2) as u32;
    let mut value_type: u32 = 0;

    let status = RegQueryValueExW(
        hkey,
        name_wide.as_ptr(),
        std::ptr::null_mut(),
        &mut value_type,
        buf.as_mut_ptr() as *mut u8,
        &mut buf_size,
    );

    if status != 0 {
        return None;
    }

    let len = buf_size as usize / 2;
    Some(String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string())
}

/// Find the GUID of the first TAP-Windows adapter, or one matching `name`.
pub fn find_tap_guid(name: Option<&str>) -> io::Result<String> {
    use windows_sys::Win32::System::Registry::*;

    let class_key: Vec<u16> = "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}"
        .encode_utf16().chain(std::iter::once(0)).collect();

    let mut hkey: HKEY = std::ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, class_key.as_ptr(), 0, KEY_READ, &mut hkey) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }

    let mut index: u32 = 0;
    loop {
        let mut subkey_name = [0u16; 256];
        let mut subkey_len = subkey_name.len() as u32;

        let status = unsafe {
            RegEnumKeyExW(hkey, index, subkey_name.as_mut_ptr(), &mut subkey_len,
                std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        if status != 0 { break; }

        let mut subhkey: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(hkey, subkey_name.as_ptr(), 0, KEY_READ, &mut subhkey) } == 0 {
            if let Some(cid) = unsafe { read_reg_string(subhkey, "ComponentId") } {
                if cid == "tap0901" || cid == "root\\tap0901" {
                    if let Some(guid) = unsafe { read_reg_string(subhkey, "NetCfgInstanceId") } {
                        unsafe { RegCloseKey(subhkey) };

                        // Optional name filter
                        if let Some(filter_name) = name {
                            let conn_key: Vec<u16> = format!(
                                "SYSTEM\\CurrentControlSet\\Control\\Network\\{{4D36E972-E325-11CE-BFC1-08002BE10318}}\\{}\\Connection",
                                guid
                            ).encode_utf16().chain(std::iter::once(0)).collect();

                            let mut conn_hkey: HKEY = std::ptr::null_mut();
                            if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, conn_key.as_ptr(), 0, KEY_READ, &mut conn_hkey) } == 0 {
                                let adapter_name = unsafe { read_reg_string(conn_hkey, "Name") };
                                unsafe { RegCloseKey(conn_hkey) };
                                if adapter_name.as_deref() != Some(filter_name) {
                                    index += 1;
                                    continue;
                                }
                            }
                        }

                        unsafe { RegCloseKey(hkey) };
                        return Ok(guid);
                    }
                }
            }
            unsafe { RegCloseKey(subhkey) };
        }

        index += 1;
    }

    unsafe { RegCloseKey(hkey) };
    Err(io::Error::new(io::ErrorKind::NotFound, "no TAP-Windows adapter found"))
}

