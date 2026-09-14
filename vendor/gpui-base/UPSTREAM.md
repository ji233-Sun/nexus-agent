# Local GPUI Base

- Source: https://github.com/longbridge/gpui-kit (crate `gpui-base` 0.6.0 from crates.io).
- Package: `gpui-base` 0.6.0.
- License: Apache-2.0; see `LICENSE-APACHE` for the upstream copyright and license.
- Local changes, all in `src/text/node.rs`:
  - A Markdown list is indented `rems(1.4)` off the paragraph column, so its
    markers no longer sit flush with the body text.
  - List items are separated by `rems(0.375)`, which upstream leaves at zero.
  - A list item's marker takes `rems(0.25)` of gap before the item content
    instead of touching it.

This directory vendors the Base rich-text renderer because Markdown list
indentation, marker spacing, and item spacing are hardcoded there and exposed
through no public `TextViewStyle` knob; upstream `gpui-base` 0.6.1 renders
lists identically. The root `Cargo.toml` patches `gpui-base` to this copy.
Update this copy deliberately alongside the lockfile; do not edit Cargo's
registry cache.
