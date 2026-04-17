# 2026-04-17 10:54:23 by RouterOS 7.18.2
# system id = HyIttZquxqG
#
/interface ethernet
set [ find default-name=ether1 ] disable-running-check=no
/interface eoip
add mac-address=FE:07:56:B2:A6:01 name=eoip-tunnel1 remote-address=\
    78.47.55.197 tunnel-id=100
add mac-address=FE:CD:86:51:70:FB name=eoip-tunnel2 remote-address=\
    78.47.55.197 tunnel-id=200
/port
set 0 name=serial0
/ip address
add address=10.255.0.2/30 interface=eoip-tunnel1 network=10.255.0.0
add address=10.255.1.2/30 interface=eoip-tunnel2 network=10.255.1.0
/ip dhcp-client
add interface=ether1
/system note
set show-at-login=no
