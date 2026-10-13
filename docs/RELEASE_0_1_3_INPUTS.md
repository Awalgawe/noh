# Windows and Linux 0.1.3 release inputs

This carries forward the reviewed [Windows 0.1.2 inputs](WINDOWS_0_1_2_REDISTRIBUTION.md)
and [Linux native materials](LINUX_REDISTRIBUTION.md). It authorizes preparation
of new candidates after Actor/Critic review; it does not qualify their final bytes.

Baseline public commit: `6829d4f07306e451ff50e6bb47468125c744e78a`. The root NOH version changes from 0.1.2 to
0.1.3. Cargo.lock SHA-256 changes from `3d71afe74c7af866e2cc1dc8eec261bdacb6f804f0b78635be05516f1f93525e`
to `3d92bd16956b2600d9b5164b16d7fcc8003177da55e3e274253d8ddd15a29f50`. Parsing both TOML documents and normalizing
only the root `noh` version yields identical package records, checksums and
dependency edges. All 486 third-party package records are unchanged.

All other reviewed native locks, sources, notices, compiler inputs, model files,
CUDA terms and NOH licence are unchanged. The only additional redistribution
input change is `assets/linux-README.md`, SHA-256 `2fc6c4e459b38fa17ab4fd33bf93b95f7b51c7add7ab16db3d19182edc685681`:
it adds the observed host chooser/default-application prerequisites and the WSLg
cross-OS drop limit. It removes no original notice or native source attribution.

The application fixes are the independently reviewed Linux UI changes merged
in PR #17. The new Windows and Linux materials archives must contain this
release's own exact NOH source tree and Cargo.lock. Previous application bytes,
source snapshots or final-byte qualification are not substituted for the new build.

One combined Windows/Linux CI build produces the portable packages. The Windows
installer stage then reuses those exact Windows application bytes for Minimal,
Standard, Complete and Web. Exact-artifact tests, independent qualification and
publication review remain required before publishing version 0.1.3.
