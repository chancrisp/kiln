# Third-party notices

Kiln is Copyright (C) 2026 Chandler Crisp. It is free software: you can
redistribute it and/or modify it under the terms of the GNU General Public
License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version. It is distributed WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. The licence text is in `LICENSE`.

This file covers
the audio libraries that Kiln's own audio engine, `kiln-media`, links or ships,
libwebp, which `kiln-core` links to write WebP, and FFmpeg, which comes with
Kiln as a separate program.
The other Rust crates built into `kiln-app.exe` and `kiln.exe` are listed in
`THIRD_PARTY_LICENSES`. Kiln bundles no fonts: it uses the ones Windows has.

Kiln's AC-3 encoder is Kiln's own code, written from the ATSC A/52 standard.
It uses no third-party AC-3 code.

| Library | Used for | Licence |
| --- | --- | --- |
| Symphonia 0.5.5 | Reading MP3, AAC, ALAC, FLAC, Vorbis, WAV, AIFF, MP4, MKV and Ogg | MPL-2.0 |
| rubato 0.16.2 | Resampling | MIT |
| libopus 1.6.1 (via opusic-sys 0.7.5) | Opus encoding and decoding | BSD-3-Clause |
| libvorbis with aoTuV and Lancer (via vorbis_rs 0.5.5 and aotuv_lancer_vorbis_sys 0.1.5) | Ogg Vorbis encoding | BSD-3-Clause |
| libogg (via ogg_next_sys 0.1.4) | Ogg container | BSD-3-Clause |
| libFLAC (via libflac-sys 0.3.4) | FLAC encoding | BSD-3-Clause |
| WavPack 5.6.0 (vendored) | WavPack encoding and decoding | BSD-3-Clause |
| libwebp 1.6.0 (via libwebp-sys 0.14.4) | WebP encoding | BSD-3-Clause |
| LAME 3.100 | MP3 encoding, in `lame_enc.dll` | LGPL-2.0-or-later |
| FFmpeg 7.1, an LGPL build | Video other than MP4, MOV, M4V and MKV, GIFs, subtitles, the video tools, and JPEG XL and AVIF, as the program in `ffmpeg\bin` | LGPL-2.1-or-later (LGPL-3.0-or-later when built with `--enable-version3`) |

## FFmpeg, LGPL

Kiln comes with FFmpeg 7.1 for 64-bit Windows, in the `ffmpeg` folder next to
this file. It is a separate program: `ffmpeg\bin` holds `ffmpeg.exe`,
`ffprobe.exe` and FFmpeg's libraries as DLLs, and Kiln starts `ffmpeg.exe` and
`ffprobe.exe` when a job needs them. No FFmpeg code is linked into
`kiln-app.exe` or `kiln.exe`. The folder can be replaced by another FFmpeg
build, and Settings > Helper apps can name a different `ffmpeg.exe`.

FFmpeg is licensed under the GNU Lesser General Public License, version 2.1 or
later. A build configured with `--enable-version3` is under version 3 or
later. The licence text is in `ffmpeg\LICENSE.txt`. This build has no GPL or
nonfree parts: it is configured without `--enable-gpl` and `--enable-nonfree`,
so it has no x264, x265 or other GPL code. Its configuration, the FFmpeg
commit and the commit of the build scripts are in `ffmpeg\BUILD.txt`.

It was built from BtbN's FFmpeg-Builds scripts
(https://github.com/BtbN/FFmpeg-Builds) at a pinned commit, target `win64`,
variant `lgpl-shared`, by the workflow `.github/workflows/ffmpeg-lgpl.yml`.
Besides FFmpeg's own code, the build includes libraries that keep their own
licences, among them these, which Kiln uses:

| Library | Used for | Licence |
| --- | --- | --- |
| libvpx | VP9 in WebM | BSD-3-Clause |
| libaom, SVT-AV1 | AV1 and AVIF | BSD-2-Clause; BSD-3-Clause-Clear, with the Alliance for Open Media Patent License 1.0 |
| dav1d | Reading AV1 | BSD-2-Clause |
| libopus, libvorbis, libogg | Opus and Vorbis sound | BSD-3-Clause |
| LAME | MP3 sound in video | LGPL-2.0-or-later |
| libwebp | Animated WebP | BSD-3-Clause |
| libjxl | JPEG XL | BSD-3-Clause |
| libtheora | Theora in OGV | BSD-3-Clause |

The full list is the `--enable-lib…` options in `ffmpeg\BUILD.txt`. Each
library's copyright and licence notices (its `COPYING`, `LICENSE` and `PATENTS`
files) are in `ffmpeg\licenses`, one file per library, and its whole source is
in the source zip below.

Source: the source of this exact build, FFmpeg's and every library's as the
build fetched them, with the scripts that built it, is
`Kiln-<version>-ffmpeg-source.zip`. It is on the same release page as the
download it came with, at https://github.com/chancrisp/kiln/releases.
`ffmpeg\SOURCE.txt` gives its address.

## LAME (MP3 encoder), LGPL

Kiln encodes MP3 with LAME 3.100. LAME is licensed under the GNU Lesser General
Public License, version 2 or later. Its text is in `LAME-COPYING.txt`, next to
this file. LAME is not part of `kiln-app.exe` or `kiln.exe`. It is built as a
separate library, `lame_enc.dll`, which sits in the same folder as the
programs and is loaded when an MP3 is made.

The DLL can be replaced. Any `lame_enc.dll` for 64-bit Windows that exports the
same LAME C functions (the list is `EXPORTS` in `crates/kiln-lame/build.rs`)
is used in its place.

The source of the LAME in the DLL is the lame-3.100 folder inside the
`mp3lame-sys` 0.1.11 crate (https://crates.io/crates/mp3lame-sys), unmodified.
LAME's own home is https://lame.sourceforge.io. How the DLL is built from it
is in `crates/kiln-lame` (`Cargo.toml`, `build.rs` and `README.md`). Both are in
`Kiln-<version>-source.zip` on every Kiln release page at
https://github.com/chancrisp/kiln/releases: LAME's source as
`third-party/crates/mp3lame-sys-0.1.11/lame-3.100`. The Rust part of
`mp3lame-sys`, which is licensed LGPL-3.0, is only declarations of LAME's C
functions, with no function bodies. It is not linked into `lame_enc.dll`: the
DLL holds LAME's C code and nothing else from that crate.

Patents: LAME's authors note that LAME may contain software that some
companies claim patents on. Where software patents are recognised, get legal
advice before deploying or redistributing LAME.

## Symphonia, MPL-2.0

Copyright (c) 2019-2022 The Project Symphonia Developers.
https://github.com/pdeljanov/Symphonia

Symphonia is licensed under the Mozilla Public License, version 2.0. Kiln uses
it with small changes to nine of its crates that stop damaged files from
crashing it; each change is marked `Kiln:` in the source. The changed source,
`crates/kiln-media/vendor/` with the list of changes in its `README.md`, is in
`Kiln-<version>-source.zip` on every Kiln release page at
https://github.com/chancrisp/kiln/releases. The unchanged source is available from the
address above and from the `symphonia` crates on crates.io. The licence text is
at https://www.mozilla.org/MPL/2.0/.

## rubato, MIT

https://github.com/HEnquist/rubato

```text
Copyright (c) 2020 Henrik Enquist

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## libopus, BSD-3-Clause

The Opus codec, bundled and built from the opusic-sys 0.7.5 crate
(https://github.com/DoumanAsh/opusic-sys). Opus is also covered by the
royalty-free patent licences named at the end of this notice.

```text
Copyright 2001-2023 Xiph.Org, Skype Limited, Octasic,
                    Jean-Marc Valin, Timothy B. Terriberry,
                    CSIRO, Gregory Maxwell, Mark Borgerding,
                    Erik de Castro Lopo, Mozilla, Amazon

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of Internet Society, IETF or IETF Trust, nor the
names of specific contributors, may be used to endorse or promote
products derived from this software without specific prior written
permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Opus is subject to the royalty-free patent licenses which are
specified at:

Xiph.Org Foundation:
https://datatracker.ietf.org/ipr/1524/

Microsoft Corporation:
https://datatracker.ietf.org/ipr/1914/

Broadcom Corporation:
https://datatracker.ietf.org/ipr/1526/
```

## libvorbis (aoTuV and Lancer) and libogg, BSD-3-Clause

Ogg Vorbis is encoded with Xiph.Org's libvorbis 1.3.7 carrying the aoTuV
beta 6.03 and Lancer patchsets, which the `aotuv_lancer_vorbis_sys` crate
bundles (https://github.com/enzo1982/vorbis-aotuv-lancer). The Rust bindings
`vorbis_rs`, `aotuv_lancer_vorbis_sys` and `ogg_next_sys` are by Alejandro
González, also BSD-3-Clause (https://github.com/ComunidadAylas/vorbis-rs).

```text
aoTuV - Copyright (c) 2003-2021 Aoyumi
Copyright (c) 2002-2020 Xiph.org Foundation

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of the Xiph.org Foundation nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

libogg, bundled by `ogg_next_sys`:

```text
Copyright (c) 2002, Xiph.org Foundation

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of the Xiph.org Foundation nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## libFLAC, BSD-3-Clause

Bundled and built from the libflac-sys 0.3.4 crate
(https://github.com/mgeier/libflac-sys), which carries Xiph.Org's FLAC
(https://xiph.org/flac/).

```text
Copyright (C) 2000-2009  Josh Coalson
Copyright (C) 2011-2025  Xiph.Org Foundation

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

- Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

- Neither the name of the Xiph.Org Foundation nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

The Rust bindings, libflac-sys:

```text
Copyright (c) 2020 Matthias Geier. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
this list of conditions and the following disclaimer in the documentation
and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors
may be used to endorse or promote products derived from this software without
specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## WavPack, BSD-3-Clause

WavPack 5.6.0, vendored in `crates/kiln-media/vendor/wavpack` (see
`crates/kiln-media/vendor/README.md` for where it came from).
https://github.com/dbry/WavPack

```text
                  Copyright (c) 1998 - 2022 David Bryant
                          All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

    * Redistributions of source code must retain the above copyright notice,
      this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above copyright notice,
      this list of conditions and the following disclaimer in the
      documentation and/or other materials provided with the distribution.
    * Neither the name of Conifer Software nor the names of its contributors
      may be used to endorse or promote products derived from this software
      without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE REGENTS OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## libwebp, BSD-3-Clause

The WebP library, bundled and built from the libwebp-sys 0.14.4 crate
(https://github.com/NoXF/libwebp-sys), which carries libwebp 1.6.0
(https://chromium.googlesource.com/webm/libwebp). Kiln uses it to write WebP
pictures. Google's patent grant for it follows the licence. The licence of
libwebp-sys itself, MIT, is in `THIRD_PARTY_LICENSES`.

```text
Copyright (c) 2010, Google Inc. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

  * Redistributions of source code must retain the above copyright
    notice, this list of conditions and the following disclaimer.

  * Redistributions in binary form must reproduce the above copyright
    notice, this list of conditions and the following disclaimer in
    the documentation and/or other materials provided with the
    distribution.

  * Neither the name of Google nor the names of its contributors may
    be used to endorse or promote products derived from this software
    without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

```text
Additional IP Rights Grant (Patents)
------------------------------------

"These implementations" means the copyrightable works that implement the WebM
codecs distributed by Google as part of the WebM Project.

Google hereby grants to you a perpetual, worldwide, non-exclusive, no-charge,
royalty-free, irrevocable (except as stated in this section) patent license to
make, have made, use, offer to sell, sell, import, transfer, and otherwise
run, modify and propagate the contents of these implementations of WebM, where
such license applies only to those patent claims, both currently owned by
Google and acquired in the future, licensable by Google that are necessarily
infringed by these implementations of WebM. This grant does not include claims
that would be infringed only as a consequence of further modification of these
implementations. If you or your agent or exclusive licensee institute or order
or agree to the institution of patent litigation or any other patent
enforcement activity against any entity (including a cross-claim or
counterclaim in a lawsuit) alleging that any of these implementations of WebM
or any code incorporated within any of these implementations of WebM
constitute direct or contributory patent infringement, or inducement of
patent infringement, then any patent rights granted to you under this License
for these implementations of WebM shall terminate as of the date such
litigation is filed.
```
