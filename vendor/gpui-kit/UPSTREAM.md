# Local GPUI Kit

- Source: https://github.com/longbridge/gpui-kit/tree/94a313a72a2513aee2780240cd322d552b2395f0/crates/kit
- Package: `gpui-kit` 0.6.0 from crates.io.
- License: Apache-2.0; see `LICENSE-APACHE` for the upstream copyright and license.
- Local changes: disable publishing. The facade source, feature flags, and export tests are retained.

This directory vendors the kit facade. GPUI, GPUI Base, GPUI Component,
and the icon assets remain registry dependencies resolved by the root `Cargo.lock`.
Update this copy deliberately alongside the lockfile; do not edit Cargo's registry cache.
