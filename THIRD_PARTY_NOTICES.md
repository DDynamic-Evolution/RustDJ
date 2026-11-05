# Third-party notices

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

## TLS transport

TLS is implemented using rustls 0.23.46 and webpki-roots 1.0.11, with ring,
rustls-webpki and rustls-pki-types. The bundled Mozilla CA roots are supplied
under the webpki-roots license. License texts for these libraries and their
new runtime support dependencies are included under third_party/licenses.
See Cargo.lock for the exact versions. rcgen is used only by local tests to
generate ephemeral certificates and is not included in the executable.
