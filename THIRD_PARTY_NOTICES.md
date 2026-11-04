# MP3 encoder notices

RustDJ includes mp3lame-encoder 0.2.5 and mp3lame-sys 0.1.11
(copyright their respective contributors), under LGPL-3.0. These packages
build and link LAME 3.100 (copyright the LAME project contributors), supplied
under its accompanying GNU Library General Public License version 2 or later.

- https://github.com/DoumanAsh/mp3lame-encoder
- https://github.com/DoumanAsh/mp3lame-sys
- https://lame.sourceforge.io/

Full license texts are included in third_party/licenses. The matching RustDJ
source package includes both MP3 library packages, including the LAME C source
and build scripts, in vendor/. It can be rebuilt and relinked with modified
versions of these libraries; see SOURCE-BUILD.md in that package.

Other Rust dependencies and their precise versions are listed in Cargo.lock;
their upstream license terms continue to apply.
