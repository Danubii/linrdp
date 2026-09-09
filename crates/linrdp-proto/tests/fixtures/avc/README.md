# Synthetic AVC420 decoder fixtures

These fixtures contain two 32 x 32 solid-red frames generated locally, with no
screen captures or third-party media. `red-idr.h264` includes SPS, PPS, an IDR,
and encoder metadata. `red-p.h264` is the following predictive frame and requires
the decoder's reference state from the IDR. Both include Annex B start codes.

RDP uses full-range BT.709 rather than the limited-range BT.601 conversion often
used by video libraries. Generate red YUV planes using the integer forward matrix
in MS-RDPEGFX 3.3.8.3.1, then encode with FFmpeg and libx264:

```sh
python3 - <<'PY'
from pathlib import Path
# R=255, G=B=0 -> Y=53, U=99, V=255 with the specified integer matrix.
frame = bytes([53]) * 1024 + bytes([99]) * 256 + bytes([255]) * 256
Path('red.yuv').write_bytes(frame * 2)
PY
ffmpeg -hide_banner -loglevel error \
  -f rawvideo -pixel_format yuv420p -video_size 32x32 -framerate 2 \
  -color_range pc -colorspace bt709 -i red.yuv -frames:v 2 \
  -c:v libx264 -profile:v baseline -pix_fmt yuv420p -qp 1 \
  -preset veryfast -tune zerolatency \
  -color_range pc -colorspace bt709 -color_primaries bt709 -color_trc bt709 \
  -x264-params 'keyint=30:min-keyint=30:scenecut=0:aud=1:threads=1' \
  -f h264 red.h264
```

Split immediately before the second access-unit delimiter (NAL type 9), retaining
its start code in `red-p.h264`. Tests allow small quantization and integer
color-conversion rounding errors. Separate direct-plane tests verify the exact
normative inverse matrix, grayscale range, and padded plane strides.
