#!/usr/bin/env python3
"""Loopback-only development fixture, not a VNC service.

Run this script, then connect LinRDP to its printed port. With --auth, enter
fixture as the test password. OpenSSL independently verifies the DES response.
Close the client window after the blue resized desktop appears.
"""
import socket,struct,time,json,sys,subprocess,zlib
s=socket.socket();s.bind(('127.0.0.1',0));s.listen(1);s.settimeout(120)
print('VNC mock port:',s.getsockname()[1],flush=True)
c,_=s.accept();c.settimeout(20)
def read(n):
 b=b''
 while len(b)<n:
  p=c.recv(n-len(b))
  if not p:raise EOFError()
  b+=p
 return b
c.sendall(b'RFB 003.008\n');assert read(12)==b'RFB 003.008\n'
if '--auth' in sys.argv:
 c.sendall(b'\x01\x02');assert read(1)==b'\x02'
 challenge=bytes(range(16));c.sendall(challenge)
 key=bytes(int(f'{b:08b}'[::-1],2) for b in b'fixture\0')
 expected=subprocess.run(['openssl','enc','-des-ecb','-provider','legacy','-provider','default','-K',key.hex(),'-nopad'],input=challenge,capture_output=True,check=True).stdout
 assert read(16)==expected,'incorrect VNC challenge response'
 print('VNC authentication verified',flush=True)
else:
 c.sendall(b'\x01\x01');assert read(1)==b'\x01'
c.sendall(bytes(4));assert read(1)==b'\x01' 
pf=struct.pack('>BBBBHHHBBBxxx',32,24,0,1,255,255,255,16,8,0)
c.sendall(struct.pack('>HH',64,64)+pf+struct.pack('>I',16)+b'LinRDP VNC smoke')
requests=0;encodings=[];sizes=set()
try:
 while True:
  kind=read(1)[0]
  if kind==0:read(19)
  elif kind==2:
   _,count=struct.unpack('>BH',read(3));encodings=list(struct.unpack('>'+('i'*count),read(count*4)));print('encodings',encodings,flush=True)
  elif kind==3:
   req=read(9);sizes.add(struct.unpack('>HH',req[5:9]));requests+=1
   if requests==1:
    pixels=b''.join(struct.pack('<I',0xff0000 if x<32 else 0x00ff00) for y in range(64) for x in range(64))
    c.sendall(b'\0\0\0\1'+struct.pack('>HHHHi',0,0,64,64,0)+pixels)
    print('sent raw red/green image',flush=True)
   elif requests==2:
    c.sendall(b'\0\0\0\1'+struct.pack('>HHHHi',16,16,32,32,1)+struct.pack('>HH',0,0))
    print('sent overlapping CopyRect',flush=True)
   elif requests==3:
    compressor=zlib.compressobj();compressed=compressor.compress(bytes([1,255,0,0]))+compressor.flush(zlib.Z_SYNC_FLUSH)
    c.sendall(b'\0\0\0\1'+struct.pack('>HHHHi',0,0,64,64,16)+struct.pack('>I',len(compressed))+compressed)
    print('sent ZRLE solid blue tile',flush=True)
   elif requests==4:
    pixels=struct.pack('<I',0x0000ff)*(80*48)
    c.sendall(b'\0\0\0\2'+struct.pack('>HHHHi',0,0,80,48,-223)+struct.pack('>HHHHi',0,0,80,48,0)+pixels)
    print('sent desktop resize 80x48',flush=True)
   else:
    c.sendall(b'\0\0\0\0')
   if requests>=100000:break
  elif kind==4:read(7)
  elif kind==5:read(5)
  elif kind==6:
   h=read(7);read(struct.unpack('>I',h[3:])[0])
  else:raise ValueError('unexpected client message '+str(kind))
except EOFError:pass
finally:
 print(json.dumps({'requests':requests,'encodings':encodings,'requested_sizes':sorted(sizes)}),flush=True);c.close();s.close()

if requests >= 6:
 assert (80,48) in sizes, "refresh requests retained the old desktop size"
