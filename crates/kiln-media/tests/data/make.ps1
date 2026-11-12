# Regenerates every audio fixture in this folder. Needs FFmpeg on PATH.
#
#   pwsh crates/kiln-media/tests/data/make.ps1
#
# The fixtures are checked in; run this only when one needs to change. Every
# clip comes from a deterministic source (a 440 Hz sine, plus pink noise with a
# fixed seed where the content should look like music), and FFmpeg runs with
# bitexact flags so a re-run writes the same bytes. Each file stays under 200 KB.
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$sine3 = @('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100:duration=3')
$sine1 = @('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100:duration=1')
$video1 = @('-f', 'lavfi', '-i', 'testsrc=size=64x64:rate=10:duration=1')
$bitexact = @('-fflags', '+bitexact', '-flags:a', '+bitexact', '-flags:v', '+bitexact', '-map_metadata', '-1')

function Make {
    param([string[]]$FfmpegArgs)
    & ffmpeg -hide_banner -loglevel error -y @FfmpegArgs
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg failed: $FfmpegArgs" }
}

# The same tone in each audio format: 3 s, 44.1 kHz, stereo unless noted.
Make ($sine3 + $bitexact + @('-ac', '2', '-c:a', 'libmp3lame', '-b:a', '128k', 'tone.mp3'))
Make ($sine3 + $bitexact + @('-ac', '2', '-c:a', 'aac', '-b:a', '96k', 'tone_aac.m4a'))
Make ($sine3 + $bitexact + @('-ac', '2', '-c:a', 'alac', 'tone_alac.m4a'))
Make ($sine3 + $bitexact + @('-ac', '2', '-c:a', 'flac', 'tone.flac'))
Make ($sine3 + $bitexact + @('-ac', '2', '-c:a', 'libvorbis', '-q:a', '3', 'tone.ogg'))
# One second of mono 16-bit keeps the PCM files small.
Make ($sine1 + $bitexact + @('-c:a', 'pcm_s16le', 'tone.wav'))
Make ($sine1 + $bitexact + @('-c:a', 'pcm_s16be', 'tone.aiff'))
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'aac', '-b:a', '64k', '-f', 'adts', 'tone.aac'))

# Lossless copies of PCM files, to check the decoders against the PCM reader bit for bit.
Make (@('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100:duration=1', '-c:a', 'pcm_s24le') + $bitexact + @('tone24.wav'))
Make (@('-i', 'tone.wav') + $bitexact + @('-c:a', 'flac', 'tone16.flac'))
Make (@('-i', 'tone24.wav') + $bitexact + @('-c:a', 'flac', 'tone24.flac'))
Make (@('-i', 'tone.wav') + $bitexact + @('-c:a', 'alac', 'tone16_alac.m4a'))

# Codecs Symphonia can demux but cannot decode.
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'libopus', '-b:a', '64k', 'tone_opus.ogg'))
# Opus in WebM and in Matroska (Symphonia demuxes, libopus decodes). The 5.1 file
# carries the same tone per channel as tone51.ac3, built from $graph further down.
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'libopus', '-b:a', '64k', 'tone_opus.webm'))
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'wavpack', 'tone.wv'))
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'dca', '-strict', '-2', '-b:a', '384k', 'dts.mkv'))
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'ac3', '-b:a', '128k', 'ac3.mkv'))
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'ac3', '-b:a', '128k', 'ac3.mp4'))

# Media Foundation's formats: WMA and raw AC3, stereo and 5.1 at 48 kHz.
# Each 5.1 channel carries its own tone (FL 440, FR 660, FC 880, LFE 55,
# BL 1100, BR 1320 Hz), so a reader can be checked for where each one lands.
Make ($sine3 + $bitexact + @('-ac', '2', '-c:a', 'wmav2', '-b:a', '128k', '-metadata', 'title=Tone', '-metadata', 'artist=Kiln', 'tone.wma'))
Make (@('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=1') + $bitexact + @('-ac', '2', '-c:a', 'ac3', '-b:a', '192k', '-f', 'ac3', 'tone.ac3'))
$tones = @(440, 660, 880, 55, 1100, 1320)
$graph = (0..5 | ForEach-Object { "sine=frequency=$($tones[$_]):sample_rate=48000:duration=1[c$_]" }) -join ';'
# The map is spelled out: left to guess, join puts a mono input on FC first.
$graph += ';[c0][c1][c2][c3][c4][c5]join=inputs=6:channel_layout=5.1:map=0.0-FL|1.0-FR|2.0-FC|3.0-LFE|4.0-BL|5.0-BR[out]'
Make (@('-filter_complex', $graph, '-map', '[out]') + $bitexact + @('-c:a', 'ac3', '-b:a', '448k', '-f', 'ac3', 'tone51.ac3'))
Make (@('-filter_complex', $graph, '-map', '[out]') + $bitexact + @('-c:a', 'libopus', '-b:a', '192k', 'tone51_opus.mkv'))

# A tagged MP3: title with non-ASCII text, track 3 of 12, a PNG front cover.
Make (@('-f', 'lavfi', '-i', 'testsrc=size=64x64:rate=1', '-frames:v', '1', '-fflags', '+bitexact', '-flags:v', '+bitexact', 'cover.png'))
$title = 'F' + [char]0x00FC + [char]0x00DF + 'e ' + [char]0x2013 + ' ' + [char]0x6771 + [char]0x4EAC
Make ($sine1 + @('-i', 'cover.png') + @(
    '-fflags', '+bitexact', '-flags:a', '+bitexact', '-map_metadata', '-1',
    '-map', '0:a', '-map', '1:v', '-ac', '2', '-c:a', 'libmp3lame', '-b:a', '128k', '-c:v', 'copy',
    '-id3v2_version', '4', '-disposition:v', 'attached_pic',
    '-metadata', "title=$title", '-metadata', 'track=3/12',
    '-metadata:s:v', 'title=Cover', '-metadata:s:v', 'comment=Cover (front)',
    'tagged.mp3'))

# A VBR MP3 with no Xing header, so its length can't be read from the file.
# The noise gives the encoder something to vary its bitrate on.
Make (@('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100:duration=8',
        '-f', 'lavfi', '-i', 'anoisesrc=duration=8:sample_rate=44100:color=pink:amplitude=0.2:seed=7') + $bitexact + @(
    '-filter_complex', '[0][1]amix=inputs=2:duration=first:normalize=0', '-ac', '2',
    '-c:a', 'libmp3lame', '-q:a', '4', '-write_xing', '0', 'vbr_noxing.mp3'))

# A CBR MP3 with no Xing or Info header, as older rippers wrote, and an ID3v1
# tag at the end: its length comes from the bitrate.
Make ($sine1 + $bitexact + @('-ac', '2', '-c:a', 'libmp3lame', '-b:a', '128k', '-write_xing', '0', '-write_id3v1', '1', '-metadata', 'title=Tone', 'cbr_noxing.mp3'))

# Video files. silent.mp4 has no audio track at all; the av.* files have one second of each.
Make ($video1 + $bitexact + @('-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'ultrafast', '-threads', '1', 'silent.mp4'))
# A silent AVI: a video whose sound only FFprobe can count.
Make ($video1 + $bitexact + @('-c:v', 'mpeg4', 'silent.avi'))
foreach ($ext in @('mp4', 'mov', 'mkv')) {
    Make ($video1 + $sine1 + $bitexact + @(
        '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'ultrafast', '-threads', '1',
        '-ac', '2', '-c:a', 'aac', '-b:a', '64k', '-shortest', "av.$ext"))
}
Make ($video1 + $sine1 + $bitexact + @(
    '-c:v', 'libvpx', '-b:v', '50k', '-pix_fmt', 'yuv420p', '-threads', '1',
    '-ac', '2', '-c:a', 'libvorbis', '-q:a', '3', '-shortest', 'av.webm'))
