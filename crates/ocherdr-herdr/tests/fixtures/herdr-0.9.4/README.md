# Herdr endpoint compatibility fixtures

The JSON fixtures are copied unchanged from `tests/fixtures/` in
[herdrdev/herdr v0.9.4](https://github.com/herdrdev/herdr/tree/7e26bad94044eba205033111a0ce8033ede7e24f/tests/fixtures),
under its Apache-2.0 license. They intentionally preserve the older version
strings in Herdr's frozen generation-one fixtures.

They verify that the client accepts the original handshake, defaults optional
features to disabled, and tolerates future snapshot fields and enum values.
The binary envelope test separately pins the tag and framing documented in
that release's `src/protocol/wire.rs`.
