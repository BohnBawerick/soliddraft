# Materials and appearances

A part has a material, which is what it is made of, and appearances, which are how its
faces look. They are two types, as in SolidWorks, Inventor and Fusion. A painted steel part
stays steel. The code is `crates/soliddraft-materials`, and the part's side of it is
`crates/soliddraft-model/src/materials.rs`.

## The two types

`Material` holds physical values in SI units, with the unit in each field's name:

| Field | Unit |
|---|---|
| `density_kg_m3` | kg/m³ |
| `youngs_modulus_pa`, `shear_modulus_pa` | Pa |
| `poisson` | |
| `yield_pa`, `tensile_pa`, `compressive_pa` | Pa |
| `thermal_expansion_per_k` | 1/K |
| `conductivity_w_mk` | W/(m·K) |
| `specific_heat_j_kgk` | J/(kg·K) |

Every value is optional. An unknown value is left out of the file. It is never written
as 0 and never guessed. A material also has an `id`, a `name`, a `category`, a `source`
(where the values come from, down to the table), a `licence`, a `default_appearance` and
a `hatch` pattern name for section views (`ANSI32` for steel, `ANSI38` for aluminium).

`Appearance` is glTF's metallic-roughness model: `base_color` (sRGB, written `"#rrggbb"`),
`metallic` and `roughness` (0 to 1), `opacity` (1 is opaque) and `emissive` (black for
none). `Appearance::shading()` gives a shader its values in linear RGB, converted with the
exact sRGB curve. Textures come later.

`problems()` on either type lists values out of range: a density that is not above 0,
a Poisson's ratio outside -1 to 0.5, a factor outside 0 to 1, a value that is not a
number. Commands refuse an entry with problems.

## Where values come from

Each value can record its source and the conditions it holds under, in the material's
`provenance` table, keyed by the value's field name:

```toml
[[material]]
id = "workshop-2024-t3"
name = "Aluminium 2024-T3"
density_kg_m3 = 2768.0
yield_pa = 3.31e8

[material.provenance.yield_pa]
source = { kind = "reference", citation = "MIL-HDBK-5J, table 3.2.3.0(b)" }
conditions = { temperature_k = 294.0, form = "sheet", treatment = "T3", direction = "longitudinal" }

[material.provenance.density_kg_m3]
source = { kind = "user_override", note = "weighed" }
```

The source `kind` is one of:

| `kind` | Fields | Meaning |
|---|---|---|
| `unknown` | | nobody recorded where the value comes from |
| `library` | `library` | taken from another library's entry, which records nothing more |
| `user_override` | `note` (optional) | typed in by the user in place of what the source gave |
| `sldmat` | `file`, `property_source` (optional) | read from a SolidWorks `.sldmat` file |
| `reference` | `citation` | a standard, handbook or data sheet, cited precisely enough to check |

The conditions are all optional: `temperature_k` (K, the bottom of the range when
`temperature_to_k` is given), `temperature_to_k` (K), `form` ("sheet", "casting"),
`treatment` ("T6", "annealed") and `direction` ("longitudinal", "along the grain"). A
condition left out is not stated. It is not assumed to be room temperature or anything
else.

A value with no entry in `provenance` has an unknown source. It is not read from the
material's `source` text, nor from the library the material sits in. Materials and parts
written before values recorded a source therefore load unchanged, with every source
unknown, and a part's copies write back the same bytes. `provenance` changes no value: mass and
everything else read the value fields as before.

`problems()` also reports an entry for a value the material does not have, and a
temperature that is not above 0 K or a range whose top is below its bottom.

In code, `Material::value(Property)` reads a value, `provenance_of` and `source_of` give
its record, `set_value` sets a value with its record and `set_override` sets one the user
typed in. Setting or removing a value replaces or drops its record, so a source never
outlives the value it describes.

The built-in library records no per-value sources yet. Each entry's `source` cites the
table its values come from.

## Libraries

A library is a TOML file:

```toml
format = "soliddraft-library/1"
name = "Workshop stock"
source = "Supplier data sheets, 2026"   # for entries without their own
licence = "CC0-1.0"                       # likewise

[[material]]
id = "workshop-6082-t6"
name = "Aluminium 6082-T6"
category = "Aluminium alloys"
density_kg_m3 = 2700.0
default_appearance = "aluminium-brushed"

[[appearance]]
id = "workshop-blue"
name = "Workshop blue"
base_color = "#1f4e8c"
metallic = 0.0
roughness = 0.4
```

The app reads libraries in this order:

1. SolidDraft's built-in libraries, compiled in and read-only
   (`crates/soliddraft-materials/libraries/`).
2. My library: `libraries/my-library.toml` in the config folder
   (`soliddraft_app::settings::user_library_path`). It is made the first time something is
   saved or imported into it.
3. The files File locations lists under `material_libraries` and `appearance_libraries`,
   in order. A team shares a library by listing the same file or folder. A file listed
   under both kinds, or listed again, is read once.

An id names one entry. When two libraries use the same id, the earlier library wins and
the catalog reports the later entry as hidden. It is never swapped in silently. Importing
gives an entry whose id is taken a new one.

## The starter library

The built-in library holds 37 materials. Every value comes from a named public source,
cited in the entry's `source`. A comment above each value in the file gives it as the
source prints it, before conversion to SI.

| Group | Materials | Source |
|---|---|---|
| Steels | AISI 1025, 4130, 4140, 4340; stainless 304, 316, 17-4PH | MIL-HDBK-5J (public domain) |
| Light alloys | aluminium 2024-T3, 5052-H32, 6061-T6, 7075-T6; Ti-6Al-4V; magnesium AZ31B | MIL-HDBK-5J |
| Copper alloys | copper C11000; brass C26000, C36000; phosphor bronze C51000 | Copper Development Association |
| | manganese bronze C86500 | MIL-HDBK-5J |
| Woods | eastern white and loblolly pine, coast Douglas-fir, northern red and white oak, sugar maple, yellow birch, black walnut | USDA Wood Handbook FPL-GTR-282 (public domain) |
| Plastics | ABS, PLA, PET, nylon 6, polycarbonate, acrylic, PTFE, polypropylene, PVC | FreeCAD material cards (CC-BY-3.0) |
| Other | glass (soda-lime), generic steel | FreeCAD material cards (CC-BY-3.0) |

How to read the values:

- MIL-HDBK-5J strengths are design minimums (A-basis, or S-basis where the handbook gives
  only that), longitudinal, at room temperature. The handbook gives no ultimate
  compressive strength, so `compressive_pa` holds the compressive yield (Fcy).
- Wood values are for clear wood at 12 % moisture content. The elastic modulus is the
  static bending modulus along the grain, compression is parallel to the grain, and
  conductivity is across the grain.
- For some plastics the card's ultimate tensile strength is the stress at break. That is
  below the yield stress for a plastic that necks before it breaks (ABS, PLA, PET,
  polycarbonate, acrylic).

Some planned materials are still missing:

- **Structural steel (A36 or S235), grey cast iron, POM, HDPE and rubber.** No source on
  the list covers them under a licence SolidDraft can ship. MIL-HDBK-5J has none of them.
  FreeCAD's cards for them are LGPL or CC-BY-SA, or do not exist.
- **MDF and birch plywood.** The Wood Handbook gives only density ranges for them, and a
  range is not a value.

The appearance library has 42 finishes: polished, machined, brushed and cast metals,
anodised colours, paints and powder coat, plastics, rubber, glass and woods. Metal colours
follow published reflectance measurements. The rest are picked to read well on screen.

## In a part

A new part has no material, and its mass reads "assign a material". `part.json` gains
three keys, each left out when empty:

```json
{
  "material": [
    { "material": "aluminium-6061-t6", "scope": "all" }
  ],
  "appearances": [
    { "on": "part", "appearance": "anodised-black", "scope": "all" },
    { "on": "feature", "feature": 3, "appearance": "anodised-blue", "scope": "all" },
    { "on": "face", "face": { "feature": 2, "generator": { "kind": "end_cap" }, "fingerprint": { ... },
      "label": "Boss-Extrude1 · End cap" }, "appearance": "paint-red", "scope": "only",
      "configurations": ["long"] }
  ],
  "snapshots": {
    "materials": [ { "id": "aluminium-6061-t6", "name": "Aluminium 6061-T6", ... } ],
    "appearances": [ { "id": "anodised-black", ... } ]
  }
}
```

- `snapshots` holds a copy of every material and appearance the part uses, by value, so a
  missing or edited library never changes the part's look or mass. `info` reports a copy
  that differs from its library, and `update` (or `Command::UpdateCopies`) takes the
  library's values. A file whose assignment has no copy is refused.
- A face is named by its persistent reference (see [modelling.md](modelling.md)), so its
  appearance follows it through edits. Every piece of a split face keeps the appearance.
  When the face is gone, or no longer faces the way it did, the assignment is reported
  as lost (`AppearanceTable::lost`) and stays in the file. It never moves to another
  face, and it applies again if the face comes back.
- The most specific appearance wins: the face's own, then its feature's, then the
  part's, then the material's default appearance, then SolidDraft's default grey.
- Every assignment has a configuration scope: `"scope": "all"`, or `"scope": "only"`
  with a list of configurations. An assignment for named configurations wins over one
  for all of them. Until configurations exist, everything is written for all of them
  and the part shows its default configuration.
- The geometry hash leaves out all three keys. A colour or material change makes a new
  revision of the part, but not a new shape.

A material can also differ per configuration, and per body when parts have several
bodies. Assemblies colour occurrences, replacing every colour of the part in that
assembly, with the same keys and scopes (see
[assemblies.md](assemblies.md#colours)).

## Mass

Mass is volume times density. The kernel measures volume in mm³, and the density is in
kg/m³, so mass in kg is `volume * density * 1e-9`. The kernel's unit-density inertia
scales the same way, to kg·mm². A part with no material, or a material with no density,
has an unknown mass, and the reason says which. `soliddraft_materials::mass::combine` adds
placed masses for assemblies: it rotates each inertia, applies the parallel-axis shift,
and returns an unknown total when any part is unknown.

## Commands

Every change is an undoable `Command` (see [modelling.md](modelling.md)):

- `SetMaterial { material, default_appearance, scope }` sets or removes the material.
  `Command::set_material_from(catalog, id)` builds it from a library entry.
- `SetAppearance { target, appearance, scope }` applies or removes an appearance on the
  part, a feature or a face. `Command::set_appearance_from(catalog, target, id)` builds
  it from a library entry.
- `UpdateCopies { materials, appearances }` replaces copies the part uses.

Each one undoes as `RestoreMaterials`, which puts back the whole previous state.

## SolidWorks libraries

`soliddraft material import file.sldmat` reads a SolidWorks material library into My
library, or into the file `--into` names. The importer reads:

| `.sldmat` | SolidDraft |
|---|---|
| each `classification` | `category` |
| each `material` | a material with an id made from its name |
| `EX`, `NUXY`, `GXY`, `DENS`, `SIGXT`, `SIGXC`, `SIGYLD`, `ALPX`, `KX`, `C` | the matching fields; the file is in SI units |
| `xhatch` | the hatch pattern's name (`ANSI38`) |
| `swatchcolor RGB` | an appearance the material names as its default, metallic when its shader is under `\metal\` |

Every value read records `kind = "sldmat"` with the file's name and the material's
`propertysource`, when the file gives one, as its source. The file states no conditions
SolidDraft keeps, so none are recorded. A property written as 0 is read as not given,
since SolidWorks writes 0 for an empty field. Properties with no SolidDraft field are listed as warnings. So is a density no
solid has (under 50 or over 25 000 kg/m³), which usually means a library written in
other units. `.sldmat` files in UTF-8 or UTF-16 both read. FreeCAD's `.FCMat` cards come
later.

## Command line

```
soliddraft material list [--category <name>] [--json]
soliddraft material show <id> [--json]
soliddraft material appearances [--json]
soliddraft material libraries [--json]
soliddraft material check <library.toml>... [--json]
soliddraft material import <file.sldmat> [--into <library.toml>] [--json]
soliddraft material info <part> [--config <name>] [--json]
soliddraft material faces <part> [--json]
soliddraft material set <part> <material-id | none> [--config <name>]
soliddraft material paint <part> <appearance-id | none> [--feature <F1 | name> | --face <n>] [--config <name>]
soliddraft material update <part> [--json]
```

`set`, `paint` and `update` save the part in place. `soliddraft part` also reports the
material and the mass.

## For the interface

The materials panel and the appearance picker build on these:

- the libraries: `soliddraft_app::settings::material_catalog()` (a `Catalog`, with
  `materials()`, `appearances()`, `libraries` and `issues`), `user_library()` and
  `Library::save`, `put_material`, `remove_material`, `fresh_material_id` and `merge`;
- "differs from library": `Catalog::match_material` and `match_appearance`;
- each value's source and conditions: `Material::provenance_of`, `source_of` and
  `set_override`, with `Display` on `Provenance`, `PropertySource` and `Conditions` for
  the text beside a field (`soliddraft material show` prints it);
- the tree's Material row and the mass properties panel: `Document::material` and
  `Document::mass`;
- the viewport: `Document::appearance_table`, which gives every face's
  `Appearance` and `Appearance::shading` for the shader;
- lost appearances: `AppearanceTable::lost`, with candidates in the same form as a
  feature's repair.
