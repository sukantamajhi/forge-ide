# Changes to codicon.ttf

`codicon.ttf` is Microsoft's Codicons font, with one addition: the letter `m` (U+006D) maps to the font's empty
`blank` glyph. GPUI's Linux text system drops any font that has no glyph for `m` (it uses that as a "is this a
text font" check), and an icon font has no letters, so on Linux every icon came out as a hex box. The icons
themselves are unchanged.
