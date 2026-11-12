# Regenerates the video fixtures in this folder. Needs FFmpeg on PATH, built
# with libx264, libx265, libsvtav1 and libopus (and its own prores_ks).
#
#   pwsh crates/kiln-media/tests/data/video/make.ps1
#
# The fixtures are checked in; run this only when one needs to change. The
# pictures are FFmpeg's testsrc2 pattern and the sound a 440 Hz sine, so a
# re-run writes the same content. x265 and SVT-AV1 run on one thread so their
# output repeats too. Each file stays under 300 KB.
#
# The HEVC and AV1 encoders here only make test data: the files are data, and
# none of these encoders ships with Kiln.
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
# SVT-AV1 prints its banner whatever FFmpeg's log level; 1 keeps it to errors.
$env:SVT_LOG = '1'

# Two seconds of 320x240 at 30 fps, and of 48 kHz sound.
$video2 = @('-f', 'lavfi', '-i', 'testsrc2=size=320x240:rate=30:duration=2')
$sine2 = @('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=2')
$bitexact = @('-fflags', '+bitexact', '-flags:a', '+bitexact', '-flags:v', '+bitexact', '-map_metadata', '-1')
# A key frame every 30 frames: two GOPs in two seconds.
$x264 = @('-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'veryfast', '-crf', '28', '-g', '30', '-threads', '1')

function Make {
    param([string[]]$FfmpegArgs)
    & ffmpeg -hide_banner -loglevel error -y @FfmpegArgs
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg failed: $FfmpegArgs" }
}

# H.264 with AAC sound, in MP4 and (with Opus sound) in Matroska.
Make ($video2 + $sine2 + $bitexact + $x264 + @('-ac', '2', '-c:a', 'aac', '-b:a', '64k', '-shortest', 'h264.mp4'))
Make ($video2 + $sine2 + $bitexact + $x264 + @('-ac', '2', '-c:a', 'libopus', '-b:a', '64k', '-shortest', 'h264.mkv'))

# The same H.264 stream with a display matrix that turns it 90 degrees
# clockwise, as a phone held upright writes it. FFmpeg's -display_rotation
# counts counterclockwise, so clockwise is -90, and ffprobe reports the matrix
# as rotation -90. (This FFmpeg ignores the old -metadata:s:v rotate=90.)
Make (@('-display_rotation', '-90', '-i', 'h264.mp4') + $bitexact + @('-c', 'copy', 'rot90.mp4'))

# HEVC, 8-bit, tagged hvc1 as Apple and Windows write it.
Make ($video2 + $bitexact + @(
    '-c:v', 'libx265', '-pix_fmt', 'yuv420p', '-preset', 'fast', '-crf', '30', '-g', '30',
    '-x265-params', 'log-level=error:pools=1:frame-threads=1', '-tag:v', 'hvc1', 'hevc.mp4'))

# HDR10: Main10, BT.2020 primaries and matrix, PQ transfer, with mastering
# display and light level metadata.
Make ($video2 + $bitexact + @(
    '-c:v', 'libx265', '-pix_fmt', 'yuv420p10le', '-profile:v', 'main10', '-preset', 'fast', '-crf', '30', '-g', '30',
    '-color_primaries', 'bt2020', '-color_trc', 'smpte2084', '-colorspace', 'bt2020nc', '-color_range', 'tv',
    '-x265-params', ('log-level=error:pools=1:frame-threads=1:hdr10=1:repeat-headers=1' +
        ':colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:range=limited' +
        ':master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)' +
        ':max-cll=1000,400'),
    '-tag:v', 'hvc1', 'hevc10_hdr.mp4'))

# AV1 in Matroska.
Make ($video2 + $bitexact + @(
    '-c:v', 'libsvtav1', '-pix_fmt', 'yuv420p', '-preset', '12', '-crf', '45', '-g', '30',
    '-svtav1-params', 'lp=1', 'av1.mkv'))

# Variable frame rate: the first second at 30 fps, the second at 15 fps (every
# other frame dropped), with the source's time stamps kept.
Make ($video2 + $bitexact + $x264 + @(
    '-vf', "select='lt(n\,30)+not(mod(n\,2))'", '-fps_mode', 'vfr', 'vfr.mp4'))

# An odd size, 885x1919. H.264 and HEVC in 4:2:0 can't carry odd dimensions
# (x264 and x265 refuse them), so this is AV1, which can, in MP4. The pattern
# is drawn one pixel larger and cropped, as testsrc2 itself rounds to even.
Make (@('-f', 'lavfi', '-i', 'testsrc2=size=886x1920:rate=10:duration=1') + $bitexact + @(
    '-vf', 'format=rgb24,crop=885:1919:0:0,format=yuv420p',
    '-c:v', 'libsvtav1', '-preset', '12', '-crf', '50', '-svtav1-params', 'lp=1', 'odd.mp4'))

# H.264 in Matroska with DTS sound, which Kiln can't read: the picture is
# still convertible, and the sound can only be copied.
Make (@('-f', 'lavfi', '-i', 'testsrc2=size=64x64:rate=10:duration=1', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=1') + $bitexact + @(
    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'veryfast', '-crf', '30', '-g', '10', '-threads', '1',
    '-ac', '2', '-c:a', 'dca', '-strict', '-2', '-b:a', '384k', '-shortest', 'dts.mkv'))

# ProRes 422 Proxy in QuickTime with AAC sound: a picture codec Windows has
# no decoder for, which Kiln hands to FFmpeg when it is installed.
Make (@('-f', 'lavfi', '-i', 'testsrc2=size=160x120:rate=10:duration=1', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=1') + $bitexact + @(
    '-c:v', 'prores_ks', '-profile:v', '0', '-pix_fmt', 'yuv422p10le',
    '-ac', '2', '-c:a', 'aac', '-b:a', '64k', '-shortest', 'prores.mov'))

# Open GOPs: the key frame at 1 s is followed in decode order by B-frames
# shown before it (leading pictures that need the GOP before), as x264
# writes with open-gop. A copy that starts at that key frame drops them.
Make ($video2 + $bitexact + @(
    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'veryfast', '-crf', '28', '-threads', '1', '-bf', '3',
    '-x264-params', 'keyint=30:min-keyint=30:scenecut=0:open-gop=1', 'opengop.mp4'))
