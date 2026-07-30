# Bundled fonts

## DejaVu Sans (`DejaVuSans.ttf`, `-Bold.ttf`, `-ExtraLight.ttf`)

Aurora bundles DejaVu Sans as its default UI font so text rendering is
deterministic and does not depend on whatever fonts a host system happens to
have installed (see ADR 0013). The Regular, Bold, and ExtraLight faces are all
bundled so a widget can select a font weight (`Style::weight` / `Style::bold`).

- Source: the DejaVu Fonts project, https://dejavu-fonts.github.io/
- License: a permissive, free license derived from the Bitstream Vera Fonts
  license plus a public-domain grant for the DejaVu changes. It allows
  redistribution and bundling. Full terms:
  https://dejavu-fonts.github.io/License.html

The font is unmodified from its upstream release.

## DejaVu Sans Mono (`DejaVuSansMono.ttf`, `-Bold.ttf`)

Bundled for the same reason, one milestone later: the code editor needs a fixed
pitch so code lines up in columns, and depending on the host having a monospace
font installed would lay the same file out differently on different machines.
Regular and Bold, so `Style::monospace()` composes with `Style::bold()` - which
is what syntax highlighting will want for keywords.

Same project, same license, also unmodified.
