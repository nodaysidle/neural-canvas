# Interface fonts

The NODAYSIDLE pair, from the nodaysidle.com assets (`Portfolio/public/assets/*.woff2`), both under the
SIL Open Font License 1.1 (licences alongside; no Reserved Font Names are declared).

- `DMMono.ttf`: DM Mono Regular, `woff2_decompress dm-mono.woff2`.
- `Manrope-Regular.ttf`, `Manrope-SemiBold.ttf`: static instances of the Manrope variable font (whose
  default instance is ExtraLight), cut with fontTools and renamed to the family "Manrope":
  `woff2_decompress manrope.woff2`, then
  `fonttools varLib.instancer --update-name-table -o Manrope-Regular.ttf manrope.ttf wght=400` (and
  `wght=600` for SemiBold), then name IDs 1/2/4/6 set to Manrope / style, IDs 16, 17, 21, 22 removed.
