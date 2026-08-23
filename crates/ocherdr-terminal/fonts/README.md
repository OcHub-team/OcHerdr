JetBrains Mono Bold (OFL-1.1) is registered process-locally so Ghostty's
`font-family-bold = JetBrains Mono` can resolve on machines that do not
have the family installed. That keeps Latin bold in the same family as
Ghostty's embedded JetBrains Mono regular, which must occupy the head of
the bold list before CJK styled fallbacks (see `write_font_families` in
`src/lib.rs`).

Source: https://github.com/JetBrains/JetBrainsMono/tree/v2.304
