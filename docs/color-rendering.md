# Terminal color rendering

On macOS, OcHerdr preserves GhosttyKit's Display P3 frame metadata and enables the
GPUI Metal renderer's optional Display P3 output. P3 terminal frames are sampled
without an intermediate sRGB gamut clamp. UI colors, gradients, glyphs, paths, and
ordinary images are sRGB and converted into the output space. Premultiplied surfaces
are unpremultiplied for gamut conversion and premultiplied again before blending.
Transparent composition uses source-over alpha.

The CAMetalLayer declares Display P3 with its sRGB transfer curve. Core Animation
color-manages that content for the destination display, including sRGB displays.
This is SDR wide-gamut support, not HDR. GPUI's default remains sRGB; Linux and
Windows continue to use the portable terminal renderer.

## Matching standalone Ghostty

Color management does not make different themes or font settings identical.
OcHerdr does not silently load or overwrite the user's standalone Ghostty config.
Use the appearance settings' **Import Ghostty Config** action to import the terminal
theme, font family, font thickening, and opacity settings. The interface theme stays
independent. Match the light/dark appearance when comparing the two applications;
different transparency and backdrops can still affect the result.

`terminal-theme` applies the selected family's background, foreground, cursor,
selection, and ANSI palette, including light/dark pairs. Explicit `background`,
`foreground`, `cursor-color`, `cursor-text`, `selection-background`,
`selection-foreground`, and `palette` assignments override terminal theme colors.
They do not recolor the application interface.

Themes imported before 0.2.12 may lack the original Ghostty background, foreground,
cursor, and selection colors. Import them again after upgrading; the original
Ghostty files are not modified.

## Regression coverage

The GPUI fork has real Metal pixel tests for P3 passthrough, sRGB-to-P3 conversion,
P3-to-sRGB fallback, premultiplied surfaces, translucent source-over composition,
paths, and color images. OcHerdr tests cover independent terminal themes, explicit
color overrides, and round-tripping imported light/dark theme colors.
