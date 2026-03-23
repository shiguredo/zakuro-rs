# Third Party Licenses

zakuro がビルド・実行時に取り込む第三者ソフトウェアのライセンスを、取り込み方の違いで 3 層に分けて記載する。

## 実行ファイルに静的にリンクされるもの

### libwebrtc

<https://webrtc.googlesource.com/src/+/refs/heads/main/LICENSE>

```text
Copyright (c) 2011, The WebRTC project authors. All rights reserved.

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

### Opus

<https://opus-codec.org/license/>

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

## 配布物に同梱する共有ライブラリ

### DuckDB

<https://github.com/duckdb/duckdb/blob/main/LICENSE>

```text
Copyright 2021-2026 Stichting DuckDB Foundation

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## 同梱せず実行時にユーザー環境が用意するもの

### OpenH264

zakuro は OpenH264 の共有ライブラリを同梱しない。`--openh264` で指定されたものを実行時に動的ロードするだけで、入手とライセンスの遵守は利用者の責任である。

<https://www.openh264.org/BINARY_LICENSE.txt>

```text
"OpenH264 Video Codec provided by Cisco Systems, Inc."
```

### FDK-AAC

zakuro は FDK-AAC の共有ライブラリを同梱しない。`--fdk-aac-lib` で指定されたものを実行時に動的ロードするだけで、入手とライセンスの遵守は利用者の責任である。

ライセンスは Fraunhofer FDK AAC Codec Library の独自ライセンスに従う。

<https://github.com/mstorsjo/fdk-aac/blob/master/NOTICE>

## Cargo 依存クレート

zakuro は多数の Cargo クレートに依存する。以下は `Cargo.lock` に記録された依存クレートと、そのライセンス (SPDX 表記) の一覧である。

<!-- BEGIN CARGO DEPENDENCIES (auto-generated) -->

| クレート | バージョン | ライセンス (SPDX) |
|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| ahash | 0.8.12 | MIT OR Apache-2.0 |
| aho-corasick | 1.1.4 | Unlicense OR MIT |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 |
| android_system_properties | 0.1.5 | MIT/Apache-2.0 |
| annotate-snippets | 0.12.16 | MIT OR Apache-2.0 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| arbitrary | 1.4.2 | MIT OR Apache-2.0 |
| arrow | 58.4.0 | Apache-2.0 |
| arrow-arith | 58.4.0 | Apache-2.0 |
| arrow-array | 58.4.0 | Apache-2.0 AND MIT |
| arrow-buffer | 58.4.0 | Apache-2.0 |
| arrow-cast | 58.4.0 | Apache-2.0 |
| arrow-data | 58.4.0 | Apache-2.0 |
| arrow-ord | 58.4.0 | Apache-2.0 |
| arrow-row | 58.4.0 | Apache-2.0 |
| arrow-schema | 58.4.0 | Apache-2.0 |
| arrow-select | 58.4.0 | Apache-2.0 |
| arrow-string | 58.4.0 | Apache-2.0 |
| atoi | 2.0.0 | MIT |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| aws-lc-rs | 1.18.1 | ISC AND (Apache-2.0 OR ISC) |
| aws-lc-sys | 0.45.0 | ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| base64ct | 1.8.3 | Apache-2.0 OR MIT |
| bindgen | 0.72.1 | BSD-3-Clause |
| bitflags | 1.3.2 | MIT/Apache-2.0 |
| bitflags | 2.13.1 | MIT OR Apache-2.0 |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| bytes | 1.12.1 | MIT |
| cast | 0.3.0 | MIT OR Apache-2.0 |
| cc | 1.4.0 | MIT OR Apache-2.0 |
| cexpr | 0.6.0 | Apache-2.0/MIT |
| cfg-if | 1.0.4 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| clang-sys | 1.8.1 | Apache-2.0 |
| cmake | 0.1.58 | MIT OR Apache-2.0 |
| combine | 4.6.7 | MIT |
| comfy-table | 7.1.4 | MIT |
| console | 0.16.4 | MIT |
| const-random | 0.1.18 | MIT OR Apache-2.0 |
| const-random-macro | 0.1.16 | MIT OR Apache-2.0 |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 |
| cranelift-assembler-x64 | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-assembler-x64-meta | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-bforest | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-bitset | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-codegen | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-codegen-meta | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-codegen-shared | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-control | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-entity | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-frontend | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-isle | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-jit | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-module | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-native | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| cranelift-srcgen | 0.133.3 | Apache-2.0 WITH LLVM-exception |
| crc32fast | 1.5.0 | MIT OR Apache-2.0 |
| crossterm | 0.28.1 | MIT |
| crossterm_winapi | 0.9.1 | MIT |
| crunchy | 0.2.4 | MIT |
| defmt | 1.1.1 | MIT OR Apache-2.0 |
| defmt-macros | 1.1.1 | MIT OR Apache-2.0 |
| defmt-parser | 1.0.0 | MIT OR Apache-2.0 |
| derive_arbitrary | 1.4.2 | MIT OR Apache-2.0 |
| duckdb | 1.10505.0 | MIT |
| dunce | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 |
| either | 1.17.0 | MIT OR Apache-2.0 |
| encode_unicode | 1.0.0 | Apache-2.0 OR MIT |
| equivalent | 1.0.2 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| fallible-iterator | 0.3.0 | MIT/Apache-2.0 |
| fallible-streaming-iterator | 0.1.9 | MIT/Apache-2.0 |
| fastrand | 2.5.0 | Apache-2.0 OR MIT |
| filetime | 0.2.29 | MIT/Apache-2.0 |
| find-msvc-tools | 0.1.9 | MIT OR Apache-2.0 |
| flate2 | 1.1.9 | MIT OR Apache-2.0 |
| fnv | 1.0.7 | Apache-2.0 / MIT |
| foldhash | 0.1.5 | Zlib |
| foldhash | 0.2.0 | Zlib |
| fs_extra | 1.3.0 | MIT |
| futures-core | 0.3.33 | MIT OR Apache-2.0 |
| futures-sink | 0.3.33 | MIT OR Apache-2.0 |
| futures-task | 0.3.33 | MIT OR Apache-2.0 |
| futures-util | 0.3.33 | MIT OR Apache-2.0 |
| getrandom | 0.2.17 | MIT OR Apache-2.0 |
| getrandom | 0.3.4 | MIT OR Apache-2.0 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| gimli | 0.33.0 | MIT OR Apache-2.0 |
| glob | 0.3.4 | MIT OR Apache-2.0 |
| half | 2.7.1 | MIT OR Apache-2.0 |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| hashlink | 0.10.0 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| http | 1.4.2 | MIT OR Apache-2.0 |
| httparse | 1.10.1 | MIT OR Apache-2.0 |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 |
| indexmap | 2.14.0 | Apache-2.0 OR MIT |
| insta | 1.48.0 | Apache-2.0 |
| itertools | 0.13.0 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| jiff | 0.2.35 | Unlicense OR MIT |
| jiff-core | 0.1.0 | Unlicense OR MIT |
| jiff-static | 0.2.35 | Unlicense OR MIT |
| jiff-tzdb | 0.1.8 | Unlicense OR MIT |
| jiff-tzdb-platform | 0.1.3 | Unlicense OR MIT |
| jni | 0.22.4 | MIT OR Apache-2.0 |
| jni-macros | 0.22.4 | MIT OR Apache-2.0 |
| jni-sys | 0.4.1 | MIT OR Apache-2.0 |
| jni-sys-macros | 0.4.1 | MIT OR Apache-2.0 |
| jobserver | 0.1.35 | MIT OR Apache-2.0 |
| js-sys | 0.3.103 | MIT OR Apache-2.0 |
| lexical-core | 1.0.6 | MIT/Apache-2.0 |
| lexical-parse-float | 1.0.6 | MIT/Apache-2.0 |
| lexical-parse-integer | 1.0.6 | MIT/Apache-2.0 |
| lexical-util | 1.0.7 | MIT/Apache-2.0 |
| lexical-write-float | 1.0.6 | MIT/Apache-2.0 |
| lexical-write-integer | 1.0.6 | MIT/Apache-2.0 |
| libc | 0.2.189 | MIT OR Apache-2.0 |
| libduckdb-sys | 1.10505.0 | MIT |
| libloading | 0.8.9 | ISC |
| libm | 0.2.16 | MIT |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| linux-raw-sys | 0.4.15 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.33 | MIT OR Apache-2.0 |
| mach2 | 0.4.3 | BSD-2-Clause OR MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| memmap2 | 0.2.3 | MIT/Apache-2.0 |
| minimal-lexical | 0.2.1 | MIT/Apache-2.0 |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| mio | 1.2.2 | MIT |
| noargs | 0.4.3 | MIT |
| noflate | 0.1.1 | MIT |
| nojson | 0.3.15 | MIT |
| nom | 7.1.3 | MIT |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 |
| num-complex | 0.4.6 | MIT OR Apache-2.0 |
| num-integer | 0.1.46 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| openssl-probe | 0.2.1 | MIT OR Apache-2.0 |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| pkg-config | 0.3.33 | MIT OR Apache-2.0 |
| portable-atomic | 1.14.0 | Apache-2.0 OR MIT |
| portable-atomic-util | 0.2.7 | Apache-2.0 OR MIT |
| prettyplease | 0.2.37 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| r-efi | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| raden | 2026.2.0 | Apache-2.0 |
| redox_syscall | 0.5.18 | MIT |
| regalloc2 | 0.15.2 | Apache-2.0 WITH LLVM-exception |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| regex-automata | 0.4.16 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| region | 3.0.2 | MIT |
| ring | 0.17.14 | Apache-2.0 AND ISC |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 |
| rustix | 0.38.44 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustix | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustls | 0.23.42 | Apache-2.0 OR ISC OR MIT |
| rustls-native-certs | 0.8.4 | Apache-2.0 OR ISC OR MIT |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 |
| rustls-platform-verifier | 0.7.0 | MIT OR Apache-2.0 |
| rustls-platform-verifier-android | 0.1.1 | MIT OR Apache-2.0 |
| rustls-webpki | 0.103.13 | ISC |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| same-file | 1.0.6 | Unlicense/MIT |
| schannel | 0.1.29 | MIT |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| security-framework | 3.7.0 | MIT OR Apache-2.0 |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 |
| semver | 1.0.28 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| shiguredo_cmake | 4.4.0 | Apache-2.0 |
| shiguredo_http11 | 2026.6.1 | Apache-2.0 |
| shiguredo_mp4 | 2026.5.0 | Apache-2.0 |
| shiguredo_openh264 | 2026.2.0 | Apache-2.0 |
| shiguredo_opus | 2026.2.0 | Apache-2.0 |
| shiguredo_toml | 2026.2.0 | Apache-2.0 |
| shiguredo_video_device | 2026.3.0 | Apache-2.0 |
| shiguredo_webrtc | 0.152.1-canary.1 | Apache-2.0 |
| shiguredo_websocket | 2026.3.0 | Apache-2.0 |
| shlex | 1.3.0 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| simd_cesu8 | 1.2.0 | Apache-2.0 OR MIT |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 |
| similar | 2.7.0 | Apache-2.0 |
| slab | 0.4.12 | MIT |
| smallvec | 1.15.2 | MIT OR Apache-2.0 |
| socket2 | 0.6.5 | MIT OR Apache-2.0 |
| sora_sdk | 2026.2.0-canary.1 | Apache-2.0 |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 |
| strum | 0.27.2 | MIT |
| strum_macros | 0.27.2 | MIT |
| subtle | 2.6.1 | BSD-3-Clause |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.3 | MIT OR Apache-2.0 |
| tar | 0.4.46 | MIT OR Apache-2.0 |
| target-lexicon | 0.13.5 | Apache-2.0 WITH LLVM-exception |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thiserror | 2.0.19 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.19 | MIT OR Apache-2.0 |
| tiny-keccak | 2.0.2 | CC0-1.0 |
| tokio | 1.53.1 | MIT |
| tokio-macros | 2.7.1 | MIT |
| tokio-rustls | 0.26.4 | MIT OR Apache-2.0 |
| tokio-stream | 0.1.19 | MIT |
| tokio-util | 0.7.19 | MIT |
| unicode-ident | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 |
| untrusted | 0.7.1 | ISC |
| untrusted | 0.9.0 | ISC |
| ureq | 3.3.0 | MIT OR Apache-2.0 |
| ureq-proto | 0.6.0 | MIT OR Apache-2.0 |
| utf8-zero | 0.8.1 | MIT OR Apache-2.0 |
| vcpkg | 0.2.15 | MIT/Apache-2.0 |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| walkdir | 2.5.0 | Unlicense/MIT |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasip2 | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasm-bindgen | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.126 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.126 | MIT OR Apache-2.0 |
| wasmtime-internal-core | 46.0.3 | Apache-2.0 WITH LLVM-exception |
| wasmtime-internal-jit-icache-coherence | 46.0.3 | Apache-2.0 WITH LLVM-exception |
| webpki-root-certs | 1.0.9 | CDLA-Permissive-2.0 |
| webpki-roots | 1.0.9 | CDLA-Permissive-2.0 |
| winapi | 0.3.9 | MIT/Apache-2.0 |
| winapi-i686-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| winapi-util | 0.1.11 | Unlicense OR MIT |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| wit-bindgen | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| xattr | 1.6.1 | MIT OR Apache-2.0 |
| zakuro | 2026.0.0 | Apache-2.0 |
| zerocopy | 0.8.55 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerocopy-derive | 0.8.55 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zeroize | 1.9.0 | Apache-2.0 OR MIT |
| zip | 6.0.0 | MIT |
| zlib-rs | 0.6.6 | Zlib |
| zmij | 1.0.23 | MIT |
| zopfli | 0.8.3 | Apache-2.0 |

<!-- END CARGO DEPENDENCIES (auto-generated) -->

### 再生成

`Cargo.toml` または `Cargo.lock` を変更したら次を実行して一覧を更新する。

```bash
python3 scripts/generate_third_party_licenses.py
```
