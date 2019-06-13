# testdet

[![Apache 2 / MIT dual-licensed](https://img.shields.io/badge/license-Apache%202%20%2F%20MIT-blue.svg)](https://github.com/hsivonen/testdet/blob/master/COPYRIGHT)

A program for testing chardetng.

## Licensing

Please see the file named
[COPYRIGHT](https://github.com/hsivonen/testdet/blob/master/COPYRIGHT).

## Dependencies

Only builds on Linux.

Requires the linker path to have `libced.a` as built from [the `ffi` branch of this fork](https://github.com/hsivonen/compact_enc_det/tree/ffi) of [compact_enc_det](https://github.com/google/compact_enc_det). (Dynamically linked GNU `libstd++` assumed.)