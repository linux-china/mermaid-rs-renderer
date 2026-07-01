# Architecture-beta diagram support

`mmdr` renders Mermaid `architecture-beta` diagrams (groups, services,
junctions, and port-directed edges).

## Layout semantics

Placement follows mermaid-js's spatial-map model rather than a force-directed
(fcose) solver, so output is deterministic:

- Edge port pairs drive relative grid placement. `db:L -- R:server` places
  `server` one cell to the **left** of `db` (the edge leaves db's left side
  and enters server's right side). `disk:T -- B:server` places `disk` one
  cell **below** `server`.
- Placement is a BFS from the first declared node, mirroring
  `shiftPositionByArchitectureDirectionPair` in mermaid-js. Same-side port
  pairs (`LL`, `RR`, `TT`, `BB`) are ignored for placement, as in mermaid-js.
- When two nodes contend for the same grid cell (a known mermaid-js
  limitation, mermaid#6120, where they render on top of each other), `mmdr`
  instead moves the later node to the nearest free cell.
- Disconnected components stack vertically.
- Group boxes are fitted around their member services after placement, so a
  group can span a 2D arrangement of services.

## Edge routing

Edges leave from the declared port side and stay orthogonal:

- Aligned same-axis ports (e.g. `R -- L` on the same row): straight segment,
  with a detour around any service sitting on the segment.
- Offset same-axis ports: Z-shaped path (two 90° bends at the midpoint).
- Mixed-axis ports (e.g. `T -- L`): L-shaped path (one 90° bend), leaving
  along the start port's axis.

## Icons

The five built-in mermaid icons are supported: `cloud`, `database`, `disk`,
`internet`, `server` (plus a `junction` marker and a symbolic `lambda`
fallback). Heuristic fallbacks map common names (e.g. `gateway`, `storage`,
`compute`) onto the nearest built-in glyph.

## Known limitations

- **External icon packs are not supported.** mermaid-js can register
  Iconify icon packs (200,000+ icons, e.g. `logos:aws-lambda`) at runtime via
  `registerIconPacks`; `mmdr` has no network access or JS runtime, so
  namespaced icons render as a neutral component glyph instead of the real
  brand artwork. AWS/logos icon packs are out of scope.
- `align row` / `align column` directives (mermaid v11.16+) are not yet
  implemented; placement relies purely on edge port topology.
- Nested groups (`group a in b`) are flattened: services are boxed by their
  immediate group only.
- Edge labels on architecture edges are not rendered.
