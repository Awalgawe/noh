# Windows 0.1.2 redistribution inputs

This supplements the [0.1.1 input review](WINDOWS_0_1_1_REDISTRIBUTION.md)
and the [original material review](WINDOWS_REDISTRIBUTION.md). It binds reused
third-party evidence to the new application version, not to untested final binaries.

The baseline is public commit `8231e922aed59a926600b739b9e5d674a9608f08`.
The previous Cargo.lock SHA-256 is `985fbef9ddfa5b8df7f12ed7211122c40c16992635ea34af0e33fdbd9de66305`;
the new value is `3d71afe74c7af866e2cc1dc8eec261bdacb6f804f0b78635be05516f1f93525e`. Parsing both TOML documents and normalizing
only the root `noh` version from 0.1.2 back to 0.1.1 produces equal documents.
All 486 third-party package records, checksums and dependency edges are unchanged.

All other redistribution inputs and the installer decoder/source lock are
byte-identical to the previously reviewed inputs. The native sources/notices,
CUDA terms, Rust sources/notices, application licence and external Microsoft
prerequisite policy retain the same scope. The 0.1.2 materials must contain its
own exact NOH source tree and Cargo.lock. This does not substitute old application
sources or qualification for new binaries.

The GitHub Release build produces the new version once. The existing installer
stage reuses those exact application bytes for Minimal, Standard, Complete and
Web. Final-byte qualification and independent publication review remain required.
