# bluey patches

This is `esp-radio` 1.0.0-beta.1 from crates.io, license MIT OR Apache-2.0, with two small
changes for the 802.15.4 scanner. They are marked `bluey patch` in the source.

1. `Ieee802154::restart_receive()` is new. `start_receive()` does nothing when the radio is
   already receiving, and the channel is only applied when receiving starts, so there was no
   way to change channel while listening. Hopping between channels needs it.
2. The driver state is reset when the radio is enabled. The line that does it was commented
   out in the port from the C driver. Without it, enabling the radio again after it was
   dropped mid-receive (leaving 802.15.4 mode and coming back) left the driver thinking it
   was still receiving.

Remove this directory and the `[patch.crates-io]` entry in the root `Cargo.toml` once
esp-radio has equivalent fixes.
