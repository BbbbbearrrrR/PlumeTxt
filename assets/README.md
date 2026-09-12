# PlumeTxt logo

`feather.png` is an AI-generated cyan feather on a transparent background. `feather.ico` contains Windows icon sizes from 16 to 256 px with transparency.

Design: a single curved feather rising diagonally, a simple geometric silhouette, three cuts, and a cyan-to-blue color transition. No text or additional objects.

Document badges reuse `feather.png` directly, preserving its silhouette and gradient. The generator only removes transparent margins and scales it proportionally; there is no separately redrawn feather.

`file-icons/` contains 93 document icons with a cyan-blue feather and colored format abbreviations, embedded as resource IDs 101–193. `file-types.tsv` maps 96 extensions to those resources. Regenerate from the repository root with `python scripts/generate-file-icons.py` (Pillow and Windows Arial Bold required); this also checks unique mappings and all icon sizes. Each ICO includes 16, 20, 24, 32, 48, 64, 128 and 256 px sizes; the app keeps its feather logo at resource ID 1.

Explorer uses copies in `%LOCALAPPDATA%\PlumeTxt\FileIcons`, named by content hash to avoid reusing old EXE resource cache entries after upgrades. Embedded icons remain available for the portable executable. Registration updates PlumeTxt's ProgIDs and extension-specific auto ProgIDs that point to this exact executable.
