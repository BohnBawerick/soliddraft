# Roadmap

SolidDraft is still being built. The current focus is reliable part modelling,
assembly documents, drawings and browser builds. This is a list of planned work,
not a release schedule.

## Modelling and documents

- Add revolve, shell, holes, patterns, mirror and reference geometry to the interface.
- Expand sketch tools with trim, extend, offset, splines and expressions with units.
- Add autosave, recovery, thumbnails and a start page for recent documents.
- Complete assembly editing, drawings of assemblies and Pack and Go.
- Add STEP, IGES and mesh import and export.
- Add configurations and in-context references after assembly references are stable.

## Interface

- Share a command registry across the ribbon, shortcuts, assistant, MCP and CLI.
- Add a command palette, shortcut editor and right-click selection tools.
- Add on-canvas feature handles, display presets, dark theme and UI scaling.
- Extend the material library with sources and conditions for individual properties.

## Reliability

- Fuzz command inputs and test damaged document files.
- Record replayable command journals for crash reports.
- Expand interface scenarios and layout checks at several window sizes.
- Move browser kernel work off the interface thread.
- Publish installers once the document formats and modelling tools are ready.

The [SolidCraft](https://github.com/bherbruck/solidcraft) project informed parts of
this backlog. Ideas do not imply copied code. Any future code reuse must keep the
upstream licence and attribution in `NOTICE`, including separate licences for its
vendored dependencies. Do not copy its screenshots, fonts or icons.
