# Patched copy of xattr 0.2.3

This is `xattr` 0.2.3 from crates.io (MIT OR Apache-2.0, see the LICENSE files) with one change in `src/sys/mod.rs`:
on Linux and Android `ENOATTR` is `libc::ENODATA`, because current `libc` no longer defines `ENOATTR` there and the
unpatched crate fails to compile.

It is pulled in by GPUI (`gpui_http_client` -> `zed-async-tar` -> `xattr = "0.2"`). The root `Cargo.toml` points
`[patch.crates-io]` at this folder. Remove both once GPUI depends on a fixed xattr.
