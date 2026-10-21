# SolidDraft interface design

SolidDraft uses a light, dense interface with a feature tree, property panel,
assistant and 3D viewport. This document records the interface and kernel boundary.

![SolidDraft modelling a bracket](screenshots/soliddraft.png)

## Layout

- Menu bar (30 px), tabbed ribbon (26 + 92 px), status bar (24 px) are fixed egui panels. Between them sit two side areas and the viewport. The left area holds the Feature tree with Properties under it (380 px by default), the right area the Assistant (360 px); each is an egui side panel with its own small `egui_dock` tree, and the middle is the dock where the drawing tab opens beside the part. A side area hides and shows with F9 (tree) or Shift+F9 (assistant), from the View menu, from the ribbon's View tab, or from the strip it leaves at its edge; it resizes by dragging its inner edge, between its minimum (200 and 240 px) and what leaves the other side and 320 px of viewport. Visibility and widths are written to `settings.json` in the config directory and restored at the next start. Window, Reset panel layout restores the default.
- The property panel lives under the tree, not in a dialog, so the viewport is never covered while a feature is edited. This is the SolidWorks PropertyManager rule and it matters more in a small window than in a large one.
- The ribbon shows every command of the first usable version, including the ones later phases deliver. Those are drawn disabled with a tooltip naming the phase. Hiding them would make the layout look finished when it is not; showing them enabled would lie.

## Colour and type

- The interface tokens are background `#f7f8fa`, panels white, ink `#1b2230`, muted `#6b7483`, lines `#d9dde4`, accent `#1f5fd0`, selection `#d7e3fb`. They are constants in `theme.rs` and every custom widget reads them from there, so the egui style, the dock style and the hand-painted widgets agree.
- Inter (Regular, Medium, SemiBold) is bundled so the three platforms look the same. egui's bundled fonts stay as fallbacks for symbols.
- Body text is 12.5 px, ribbon labels 11 px, group labels 10.5 px. Dense, but every label is a real word, never an abbreviation.
- The viewport background is a light vertical gradient; a part with no appearance assigned is the blue-grey of `Appearance::default_part`, lit by a key, fill and rim that follow the camera so curved faces read without edges. Hover tints a face 22 percent toward the accent, selection 55 percent, mixed in after tone mapping so they look the same in every look. Edges are 90 percent ink, 1.3 points wide, pulled slightly toward the camera so they win the depth test on their faces.

## Icons

All icons are drawn with the egui painter on a 16 by 16 grid (`icons.rs`). No icon font and no third-party artwork, so there is nothing to license and the set scales crisply with the UI. The CAD glyphs (extrude, revolve, fillet, chamfer, shell, pattern) are original drawings of the operation itself, not copies of any vendor's icon.

## Viewport and navigation

- Z is up. Front is the viewer standing on -Y looking at +Y; Top looks down -Z with +Y up on screen; Right looks along -X. Isometric is the viewer at (+X, -Y, +Z), the SolidWorks default.
- Left drag orbits, right or middle drag pans, Shift + left drag pans, the wheel zooms toward the cursor. A click with no drag selects. Left drag for orbit keeps a laptop trackpad usable; SolidWorks users who expect middle drag get pan, which is the less destructive surprise.
- View changes animate over about 150 ms so a keyboard user can see which way the model turned.
- The view cube in the top right is drawn with the painter from the camera's rotation. Visible faces are clickable; the hovered face highlights.
- The heads-up toolbar is a single floating strip: isometric, fit, F, T, R, then display style. Section and measure are present but disabled until phase 2.
- An axis triad in the lower left, a units and view pill next to it, and a hover label in the lower right that names what is under the cursor.

## Picking

Two mechanisms handle picking:

1. An id buffer. The scene is rendered a second time, without multisampling, into an `Rg32Uint` target. The second channel is the entity: faces write `1 << 30 | face id`, edges `2 << 30 | edge id`, vertices (expanded to 7 px squares) `3 << 30 | vertex id`. The first channel is 0 for the open part and, for an assembly, the occurrence's row plus one (see [assemblies.md](assemblies.md#rendering-and-picking)). Every frame a 64 by 32 texel window around the cursor is copied to a mappable buffer. The copy is read the next frame, so hover lags one frame; while the pointer rests on the view the app repaints once more to close that gap.
2. Priority in the window: nearest vertex within 7 px, else nearest edge within 5 px, else the face under the cursor. That is what makes a corner or an edge clickable without aiming at one pixel.
3. A CPU ray against the tessellation of the picked face, edge or vertex gives the exact 3D point for the status bar and for later features (sketch on face, measure).

## Render looks

The viewport has three looks, render settings rather than display modes: **Fast** (appearance colours, metallic-roughness shading, the studio environment, tone mapping, edges), **Studio** (Fast plus soft shadows and light ambient occlusion; the default) and **Realistic** (a stronger shadow light, stronger reflections and occlusion, edges off). The View menu and the command palette switch them; `--preset` on the command line and `?preset=` in the web build's address start in one; `settings.json` holds the default under `view.preset`. `viewport/look.rs` has the values.

- **Shading.** Each face vertex carries an index into its body's appearance table (`soliddraft-materials`' `Shading`, up to 64 per body), filled from the part's own assignments as the model resolves them (`appearance_table`; see [materials.md](materials.md)). Changing an appearance (`Document::set_appearance`) recolours the meshes without asking the kernel for anything. Lighting is linear and metallic-roughness; the environment is a procedural studio (a grey room, an overhead softbox, two strip lights) baked at startup into a 64-texel cube map prefiltered for roughness and nine spherical harmonics for diffuse light (`viewport/environment.rs`), so metals have something to reflect in every look. Each fragment is tone mapped once with Khronos PBR Neutral.
- **Shadows.** A light fixed above the model, tilted toward the back left of the isometric view so shadows fall forward where they show, casts them onto the parts and onto a ground disc under the model's lowest point. It does not follow the camera, so the 2048 texel shadow map is redrawn only when the geometry changes, not while orbiting; the studio rig that follows the camera stays unshadowed. The ground only darkens: it is invisible where nothing shades it and fades out toward its rim and toward a grazing view.
- **Ambient occlusion** is a depth and normal prepass, twelve hemisphere samples a pixel and a depth-aware blur.
- **Depth is reversed** on every camera pass (near 1, far 0, an infinite far plane in perspective) for even precision over a float depth buffer.
- **Edges are the kernel's**: `tessellate_edges` samples each B-rep edge along its exact curve, and each segment is an instance drawn as a quad of constant pixel width, since WebGL2 draws only one-pixel lines. Seams are not drawn. The mesh's density never changes which edges exist.
- **Tiers.** WebGPU, Vulkan, Metal and Direct3D 12 run every pass. WebGL2 (and native OpenGL) runs the same shading, shadows and wide lines without the occlusion pass; everything else uses only what WebGL2 has (no storage buffers, no compute).

The colour pass renders into offscreen targets in the callback's `prepare` step and is blitted into egui's render pass in `paint`. This keeps the 3D pipeline independent of how eframe configures its own pass (sample count, depth, format), and the blit shader chooses linear or gamma output from the swapchain format at startup.

## Kernel boundary

The app talks to `soliddraft_kernel_api::Kernel` and nothing else. The backend is chosen in one place, `soliddraft_app::kernel()`, which hands out an `Arc<dyn Kernel>` on the SMLib backend; the document, the viewport and the panels never name a backend.

The document replays its feature list on every change: each extrude is `create_body_from_profile` plus `extrude`, the sheet body is released at once, the previous solids are released before the replay. Kernel ids change on every replay, so a selection is carried across a rebuild by name: the same feature and the same label ("Boss-Extrude1 · End cap"). The labels come from the app, not the kernel: for an extrude, a planar face whose centroid lies on the sketch plane is the start cap, one at the extrusion distance is the end cap, a cylindrical face is the wall and the rest are sides in topology order.

Three things the interface needs were not in the trait in phase 1, so the shell derived them from the labelled display mesh. Phase 2 added all three to the trait (`tessellate_edges`, `VertexId` with history, and `FaceRole` on created faces; see [kernel-bridge.md](kernel-bridge.md)), and the `soliddraft-model` document names faces from them ([modelling.md](modelling.md)). Face names now come from the model. The viewport draws the kernel's sampled edges (`tessellate_edges`) and takes corners from the topology's vertices where three or more faces meet, keyed by feature and ordinal, so a corner has no identity across operations yet. One heuristic is left:

- **Face roles.** History says which faces an operation created but not what part of the operation made them. The naming above is a heuristic that holds for built-in profiles centred on the plane origin; a role or a parent-segment reference on created faces would make it exact and would survive the sketcher.

## Assistant panel

The model is not connected in phase 1. The panel is the real layout: system note, user and assistant bubbles, a proposal card with Preview, Apply and Discard, and a composer with context chips. The placeholder proposal is live: Preview ghosts the geometry, Apply runs the same `Command::AddExtrude` the ribbon would and marks the feature "via assistant" with a spark in the tree. Typed messages get a reply explaining that the assistant arrives in phase 3.

## Shortcuts

F, T, R, I views, N normal to the sketch plane or the selected face. Z zoom to fit. S sketch, E extrude, X cut, F or C fillet or chamfer the selected edge. Enter and Esc apply or cancel the open feature and leave a sketch. Delete removes the selected feature or sketch entity, F2 renames. Ctrl+Z and Ctrl+Y undo and redo, Ctrl+S and Ctrl+O save and open, Ctrl+N new part, Ctrl+Q quit. In a sketch, L R C A S P D pick the line, rectangle, circle, arc, slot, point and smart dimension tools and X toggles construction. / focuses the assistant, ? opens the shortcut list. F9 and Shift+F9 hide or show the feature tree and the assistant.

## Automation

`--screenshot PATH` saves the window through egui's screenshot command and exits; `--bench` loads a body tessellated to over 10,000 triangles, disables vsync, orbits continuously and prints mean, median, p95, p99 and max frame times. Both exist so the exit test can be rerun on any machine without a human at the keyboard.

## Phase 2: the sketcher

The sketcher draws on the 3D view with the egui painter; the sketch plane maps points both ways through the camera, so there is no second renderer and the sketch sits exactly on the face it was drawn on.

- **Entities own their points.** A line has two, an arc a centre and two ends, a circle a centre and a radius. Points of different entities are joined by coincident relations, the way SolidWorks and the solver model it, so an entity can be deleted without rewriting its neighbours. Joined points are drawn filled, free ends hollow, so an open corner is visible before the solver says so.
- **Inference is what the user sees.** While the pointer moves, a point within 8 px snaps to an existing point, the middle of a line or a curve; a new line within 6 px of level or plumb with its start snaps exactly. Every snap shows its glyph by the cursor, and every snap that lands a click becomes a relation. Nothing is inferred silently.
- **Tools are chains.** Line continues from its own end until a click lands on an existing point, a right-click or Esc. Rectangle, circle, centre-point arc, straight slot and point take two or three clicks. The slot is built from two lines, two arcs, four coincident, four tangent and one equal relation plus a construction centreline, the same relations the user would add by hand, so it stays a slot when dragged.
- **Smart dimension places by position.** One pick and a placement click. A line dimensioned above or below it gets its horizontal extent, left or right of it the vertical one, inside its box the length; two points work the same way; two lines give an angle, or a distance when parallel; an arc gives a radius and a circle a diameter. The value pill opens for typing at once, and a dimension is a relation from the moment it is placed.
- **Three colours.** Entities that can still move are accent blue, fully defined ones ink black, the ones in a conflicting relation red, with the status pill naming the state and the number of conflicts. The solver's own answer drives this: which variables are still free and which relations it could not satisfy.
- **The solver is one function.** `sketch::solve::solve` (in the `soliddraft-sketch` crate, which also holds the sketch model and its profiles) turns the sketch into ezpz requests and writes the answer back; everything else sees only the sketch and its status. Dragging a point adds a low-priority wish for its position, so the real relations win and the rest of the sketch follows. Signed distances keep the side the user drew on.
- **Sketches on faces** use a frame with u to the right and v up when the face is looked at from outside, and the model origin projected onto the face as origin. A sketch therefore does not move when the face's outline changes, and a bore sketched on a boss reads as x, y of the part.
- **Entering a sketch** looks straight at the plane, switches to orthographic, and fits the body or, with nothing built yet, a 200 mm workspace. Leaving it restores the projection. The ribbon jumps to the Sketch tab and back.

## Phase 2: one body, references and repair

- **One running body.** Each extrude merges with the body so far, each cut subtracts a tool through it, fillets and chamfers round its edges. A cut defaults to going against the sketch normal, into a face; a boss along it. "Through all" reaches past the far side of the body.
- **Faces are named by the feature that made them and the part of it that did**: start cap, end cap, the side of a given sketch line or arc, the wall of a circle, the blend of an edge. The names come from the model's persistent names (see [modelling.md](modelling.md)); edges are numbered per feature. The hover label, the property panel and the repair messages all use these names.
- **References are origin plus fingerprint.** A face is named by the feature that generated it, its generator and an ordinal; an edge by the two faces it separates. On rebuild the names are followed through the kernel history; a base made thicker moves the inside corner and the fillet follows it. When a name finds no entity, several, or one whose type or facing changed, the feature stops with a repair request instead of guessing, the tree marks it, and the body passes through unchanged so the rest of the part still builds.
- **Repair is a pick.** The property panel lists each missing reference with a strike-through, the viewport takes the replacement click, and OK rewrites the reference. The feature then rebuilds like any other.
- **Editing rolls back.** Opening a feature for editing rolls the display back to the body before it, so the edges a fillet works on are the ones you can click. The rollback bar shows where the display stands; commit or cancel rolls forward again.
- **Previews replace the body in the colour pass only.** A fillet or cut preview is drawn in place of the body, tinted toward the accent, while picking still runs on the original. Added material is a blue ghost, removed material a red one. Picking the edges of a fillet keeps working while its preview is on screen.
- **Undo is the model's command stack**: every panel command becomes one model command, or one batch of them, labelled by the panel's own words, so the Edit menu says what it will undo. Kernel state is never part of it; undo applies the exact inverse and rebuilds from the first feature it changed.
- **The tree nests an absorbed sketch under its feature**, marks suppressed features and the ones past the rollback bar in grey, and shows a red dot for an error and an amber triangle for a missing reference. A second click on a selected row renames it; dragging a row moves it; the bar between rows is the rollback bar.
- **Files are the model's part files** (`.dwprt`, see [modelling.md](modelling.md); `.dwpart` files still open and save as `.dwprt`); the File menu, Ctrl+S and the recent-files list work on them. Recents live in the platform configuration directory.

### The document behind the panels

The part is `soliddraft_model::Document`: features, commands, undo and redo, rebuild, persistent references and files. The app's `document.rs` is the layer the panels read: it turns their commands into model commands, mirrors the model's tree as the feature list after every change, and keeps what only the interface needs (display meshes, picking, selection, previews).

## Phase D2: drawings

A drawing is its own document that opens as a tab beside the part.

### Documents and the shell

- **Make drawing** sits in a Drawing group on the part's Features tab (also File, New drawing from part, Ctrl+Shift+D). It opens the New drawing PropertyManager in the property panel: template, sheet scale (auto fits the part), projection angle from the template, units, and the views to place as tiles with Front, Top, Left and Isometric ticked. Create drawing projects the ticked views at scale 1, picks the largest template scale at which the orthographic group fits beside the pictorial, places them (first angle: top below front, side view to the right; third angle the other way), gives the pictorial the next smaller scale, inserts centre marks for every circle seen face-on and the template's general notes, and opens the tab.
- **One window, two documents.** The drawing tab lives in the same dock leaf as the part; whichever is active drives the ribbon, the tree panel, the property panel, the status bar and the shortcuts. The hooks into the shell are small on purpose: `Tab::Drawing` in `app.rs`, one arm at the top of `ribbon.rs`, one `AppAction::Drawing` variant and one `DrawingState` field in `state.rs`. Everything else is under `src/drawing/`.
- **The drawing has its own undo stack**, a snapshot per command labelled by it, like the part's. Ctrl+Z goes to whichever document is active.
- **Status bar:** sheet number, size, scale, projection angle, view count and "Up to date with Bracket rev 3f2a9c1b" (the first digits of the part revision the drawing is pinned at), or the number of dangling and changed annotations, or that the views are older than the part. The document tab carries a dot while the drawing has stale views or annotations needing attention.

### Views, alignment and placement

- Views fold off a selected view: eight direction dots around the parent, the orthographic four give folded views (the sheet's projection angle decides which face: in first angle, right of Front gives the Left view) and the diagonals give pictorials. The ghost of the view follows the pointer, dashed accent guides show the alignment that will be kept, a hover card names the resulting orientation and says to hold Ctrl to break alignment. The projection for each direction is computed once per direction and cached while the tool is active.
- An aligned view slides only along its guide; dragging a parent drags its aligned children; Break alignment and Align are in the Arrange group, the context menu and the view's PropertyManager.
- Scale per view: sheet, parent or custom; changing the sheet scale rescales the views that follow it without going back to the kernel (an affine on the cached paths and annotations). "SCALE 1:2" is captioned under views whose scale differs from the sheet.
- Hidden lines are kept in the cached geometry and switched on and off per view by the display style, so no re-projection is needed. Tangent edges are passed to the kernel as a request; the backend does not yet tell smooth edges apart in its output, so they draw like other edges.

### Dimensions and annotations

- **Smart dimension** reads the pick, as the sketcher does: one line gives its length, a circle a diameter, an arc a radius, two parallel lines a distance, two other lines an angle, a round and a line the distance from the centre, two ends a point-to-point distance. Clicking near the end of a curve picks that end. The placement decides a linear dimension's orientation: above or below the span measures horizontally, beside it vertically, inside it along the line. Picking rule when curves overlap: annotations win over curves, visible curves over hidden ones, and of two curves at the same distance the shorter one, so a hole's edge wins over the long edge it lies on.
- The **dimension palette** opens on the selected dimension: symmetric or deviation tolerance with its value, precision, diameter or radius, parentheses and the text. Every change is one command on the undo stack.
- **Hole callout** reads the hole feature: count of holes of that size in the feature (by distinct centre, since the far circle of a through hole lies under the near one), diameter, THRU for a through-all cut or the depth symbol with the depth. **Centre marks** are inserted automatically on view creation and on demand. **Notes** take a leader when placed on an edge and are typed in the property panel. **Model items** imports extrude depths into a view that sees the extrusion edge-on and circle diameters into a view that sees the circle face-on, skipping what the sheet already shows; sketch distance dimensions are not imported yet.
- The PropertyManager of a dimension lists what each end references in the part's own words (feature, role, edge or face, which point of it), so a user can predict what survives a change.
- Dimension text uses the template's style file: ISO places the value above an unbroken line with filled arrowheads; ASME breaks the line. Text is laid out by cap height (ISO 3098 lettering heights) so 3.5 mm on the sheet is 3.5 mm.

### When the part changes

- Both documents are in one window, so the drawing sees every rebuild. With automatic update on (the default, stored in the drawing), the views regenerate and every annotation re-resolves its references; while a feature editor is open or the part is rolled back the update waits, because the displayed body is not the part. With it off, the views keep their geometry, their boundary turns hatched, the status bar says so and Update views (U) regenerates.
- **References are the part's own.** An anchor carries the model crate's persistent name for its entity (a face by feature, generator and ordinal; an edge by the two faces it separates; both checked by fingerprint), the same name a fillet stores for its edge, so a drawing dimension follows an edge the way a fillet does: a plate made 2 mm thicker moves the end edge and the dimension reads 12. The drawing type's placeholder reference gained one optional field for it; its words (feature and role) stay beside it so a lost reference can still be described after its feature is gone. A face's silhouette is found again as the nearest silhouette of that face in the new view.
- **Nothing is deleted silently.** A reference that resolves to nothing leaves the annotation on the sheet in red with an × at the lost end and a card in the **Needs attention** list with the cause in words ("Both references were edges of Slot, which no longer exists") and three repairs: Reattach (one pick per lost end), Delete, Keep as note (the text frozen with a reference mark). A value that moved beyond its shown precision is highlighted on the sheet and listed under Changed values until accepted. The banner over the canvas says what changed in the part, how many views updated, how many values changed and how many dimensions lost their references, with Review and Undo update (one undo step).
- The whole update is one command: undoing it restores every view's geometry and every anchor, and the drawing is then older than the part again.

### Templates and styles as data

The sheet formats and drafting styles are TOML files a drawing office can edit, in `crates/soliddraft-drawing/templates/` (ISO A3 and A4 first angle, ANSI A and B third angle: paper, margins, zone grid, ISO 128 line widths, scales, the title block as rules and fields with `${property}` links, the projection symbol, default general notes) and `crates/soliddraft-drawing/styles/` (ISO 129-1 and ASME Y14.5 dimension styles: text heights and placement, arrows, extension line gaps and overshoots, symbols, centre mark sizes; hatch patterns; arrowhead definitions; sheet fonts). They are compiled in and parsed once; the drawing crate's `library` module turns each into the model's `Template`.

### Export

PDF, DXF and SVG are written from the same display list the canvas paints, so what you see is what you export. The PDF is one page per sheet at the paper size with vector paths, widths and dash patterns, and its text in an embedded subset of Inter with a Unicode map, so it can be searched and copied (every weight prints as Regular). The DXF is R2000 in millimetres with LINE, ARC, CIRCLE, SOLID (arrowheads) and TEXT entities on VISIBLE, HIDDEN, PHANTOM, CENTER, DIMENSION, TEXT, HATCH and FORMAT layers with line types; curves go through a lines-and-arcs fit within 0.01 mm, so it has no splines. The SVG is the paper in millimetres.

### Where the code lives

Projection, sheet layout and auto scale, alignment, measurement, Model items, associativity with its repairs, the commands and undo stack, the display list, the template library, the exporters and the drawing file are in `crates/soliddraft-drawing`, shared with the headless `soliddraft draw`. The app adds two thin modules: `drawing/part.rs` presents the part document as the crate's `Part` and names entities with the model's persistent references (and reads hole features for callouts), and `drawing/document.rs` holds the open drawing with its undo stack, file path and the views' transforms for picking. A drawing file is a zip (`.dwdrw`; `.dwdrawing` files still open) with a manifest, the drawing, each view's cached line work and a thumbnail; its part reference is described in [documents.md](documents.md). Updates still run on the interface thread (about a second for four views of the bracket on the SMLib backend).

### Screenshots


## Phase D3: the rest of the view set and the tables

Section and detail tools follow SolidWorks's gestures. Every view and symbol is made by `soliddraft-drawing` (`derived`, the symbol rules, the commands); the app collects clicks, previews the result and pushes commands.

### Derived views

- **Section**: the first click on a view starts the cutting line (circle centres snap, so a cut through a boss lands on its axis); moving the pointer picks horizontal or vertical; the second click finishes the line, and the kernel's section is projected at once. The ghost then follows the pointer along the alignment the projection angle gives (first angle: where the arrows point; third angle: where the eye is); F flips the direction and recomputes, Ctrl frees the view, the third click places it. The letter comes from the drawing's counter (I, O and Q skipped). The view's PropertyManager edits the letter, flips the direction, picks the hatch pattern (the library's `hatch.toml`), and can redraw the cutting line with the same two clicks.
- **Detail**: a click for the centre and one for the radius, or, with Profile chosen, a click per corner closed on the first corner, a double-click or Enter. The magnification (2× the parent by default) and the sheet the view goes to are chosen in the PropertyManager; a detail sent to another sheet is placed there at once and the canvas switches to it. Magnification after placing goes through the view's scale, so its annotations rescale with it.
- **Auxiliary**: click an edge; the view unfolds square to it from the side away from the parent's centre; F looks from the other side.
- **Crop** takes two corners; **Broken-out** a centre, a radius and a depth from the PropertyManager. Both and their removal are on the view's PropertyManager, as are the view's location labels (sheet and zone of the view and of its mark) and a Sheet choice that moves the view to another sheet at a free place.
- Hidden lines and tangent edges are per view in the PropertyManager and in a Display group on the Views tab; a tangent edge change remakes that one view through the kernel rather than updating the whole drawing, and the view's transform is remembered again so later picks on it land.

### Dimension sets and symbols

- **Ordinate, baseline and chain**: pick the origin and the points (edge ends and circle centres snap, an edge's middle otherwise), then a click away from the edges places the set at that distance. Points on one line take the direction that fits them; otherwise the PropertyManager's choice holds. The values are measured from the picks' cached points at the view's scale and re-resolved by the crate on update like any anchor.
- **Geometric tolerance** is built in the PropertyManager before placing: characteristic, zone (diameter, value, material condition) and up to three datum references with modifiers, with the frame's text previewed. A click on an edge anchors it with a leader, the next click places it; a click away from the edges stands it free. **Datum** and **surface finish** need an edge; **revision symbol** marks the drawing's current revision unless another is typed. Every symbol's PropertyManager edits it afterwards through `EditAnnotation`.
- **Revision table**: the Annotate tab's Revision table and the tree row open the panel: the rows so far and a form for the next (revision defaults to the next letter, date to today). The first row makes the table at the top right of the drawing area; the drawing's revision follows the row; the ZONE column lists the zones of the symbols that carry the revision. The table is selectable on the sheet (it has its own display owner now).

### Sheets

Sheet tabs under the canvas, as SolidWorks shows them: click to switch, double-click or the context menu to rename, Move left and right to reorder, Delete when no view elsewhere is made from the sheet's views, and a plus for a new sheet from the first sheet's template. The sheet PropertyManager has the same. Reordering needed one plumbing addition, `Command::MoveSheet`; views keep their ids, so location labels pick up the new sheet numbers by themselves. Export: the PDF now carries every sheet as a page (`bracket.pdf`); DXF and SVG stay one sheet each.

### Screenshots


## Phase 0: documents and references

### A drawing and its part

- A drawing is its own file that references its part by id and pins the revision it was saved against. Opening it shows that revision whatever the part has become since; the part's current revision comes from the open part or from its file, and the difference is the **newer version available** marker: in the drawing tree (the model row under the document carries the SolidWorks out-of-date glyph and the pinned revision), the status bar, a bar over the sheet, the hatched view borders and the Model reference panel. Nothing updates by itself on open.
- **Rebuild** is where SolidWorks keeps it: Edit > Rebuild drawing (Ctrl+B), the Model group of the Views tab, the bar over the sheet. It brings the views to the current part, re-resolves every annotation, re-pins the reference and binds it to where the part was found, as one undo step. When the part is not open, Rebuild opens its file in the part tab first.
- **A missing part is repaired, never guessed.** The repair dialog says what was wrong at the stored path and that nothing with the id was in reach, lists the parts in reach as cards in the Needs attention language (the same name first), takes a path by hand, and links to File locations. A same-named file with another id is a candidate, not a match; taking it is Replace model. The pinned revision stays, so the sheet keeps showing what it showed.
- **File locations** (Tools menu) is one page for every kind of reference file: material and appearance libraries, part, assembly and drawing templates, sheet formats and title blocks, drafting styles, configuration tables, and the folders searched for moved references. Built-in content is listed read-only; the user's folders and files are added, removed and reordered under it. The schema and the lookup order are in [documents.md](documents.md).

### Screenshots
