# 2026-04-17 10:54:22 by RouterOS 7.18.2
# system id = jBOToAUgXWO
#
/interface ethernet
set [ find default-name=ether1 ] disable-running-check=no
set [ find default-name=ether2 ] disable-running-check=no mtu=1450
/interface eoip
add mac-address=FE:7D:41:89:DF:2A name=eoip-tunnel1 remote-address=\
    128.140.114.175 tunnel-id=100
add mac-address=FE:64:26:2F:11:23 name=eoip-tunnel2 remote-address=\
    128.140.114.175 tunnel-id=200
/port
set 0 name=serial0
/ip address
add address=10.0.0.2 interface=ether2 network=10.0.0.2
add address=10.255.0.1/30 interface=eoip-tunnel1 network=10.255.0.0
add address=10.255.1.1/30 interface=eoip-tunnel2 network=10.255.1.0
/ip dhcp-client
add interface=ether1
/ip route
add dst-address=10.0.0.1/32 gateway=ether2
add dst-address=10.0.0.0/16 gateway=10.0.0.1
/system note
set show-at-login=no
