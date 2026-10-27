# The assistant

Phase 3 connects the chat panel on the right to a model, and serves the same commands to outside agents over MCP. The assistant changes the part only through the command layer the ribbon uses, so everything it does is an ordinary feature in the tree that the user can undo.

## Crates

`soliddraft-assistant` holds everything that does not know about documents:

- `provider`: the app-owned `Provider` trait. `AnthropicProvider` calls `POST /v1/messages` with reqwest and reads the server-sent events itself, since Rust has no official Anthropic SDK. `OpenRouterProvider` sends the same request to OpenRouter (see [Providers](#providers)). `MockProvider` plays back scripted or recorded streams through the same reader.
- `choice`: which provider, key and model to use, from the settings, the environment and the keys present.
- `conversation`: the agent loop. It sends the user's text, streams the reply, runs each tool call against a `ToolHost`, and loops until the model stops calling tools. Each request starts with `Provider::start`, which runs it on a worker thread on the desktop; the app polls once per frame and runs tool calls on the interface thread, where the document lives.
- `tools`: tool definitions, schema builders for the strict subset, and the validator every tool input passes before it runs.
- `mcp`: the Model Context Protocol server, on stdio or on Streamable HTTP, plus a small client for tests.
- `key`: each provider's API key, from its environment variable or the OS keychain.
- `relay`: the line format for a provider on a server. The web build sends its requests to `soliddraft-server`, which runs them on its own provider and key (see [docs/web.md](web.md)).

`soliddraft-app` implements `ToolHost` over its document in `command_tools.rs`, and over the whole app state with its drawing in `app_tools.rs`, and drives the conversation and the MCP endpoint from `chat.rs`.

## Proposals

A proposal is a list of recipe lines plus a one-line summary. A recipe line is one command-layer verb with every reference by name. The sample hole pattern, for example, is two lines:

```json
[
  { "op": "sketch", "plane": "Boss-Extrude1 · End cap", "shapes": [
    { "shape": "circle", "center": [15, 28, 10], "diameter": 9 },
    { "shape": "circle", "center": [105, 28, 10], "diameter": 9 },
    { "shape": "circle", "center": [15, 72, 10], "diameter": 9 },
    { "shape": "circle", "center": [105, 72, 10], "diameter": 9 }
  ]},
  { "op": "cut", "sketch": "previous", "through_all": true, "depth": 10, "flip": false }
]
```

Sketch points are world coordinates on the sketch plane, so the model never converts between frames. A rectangle, circle or slot becomes the same entities, relations and dimensions the sketcher's tools would draw. `previous` names the sketch the nearest earlier line made.

`propose` runs the lines on a scratch copy of the part before anything else. The copy is a fork that shares the part's feature builds, so it builds only what the lines change. A line whose reference does not resolve (a feature, face or edge name that does not exist, a point off the sketch plane, `previous` with no sketch before it) is rejected with the reason, and so is a result that breaks a feature that built before. The model gets the reason back and can fix the lines. An accepted proposal shows as a card with Preview, Apply and Discard. Preview draws the part as it would be after the proposal.

Apply runs the same commands as the ribbon, as one step on the document's undo stack (`soliddraft_model::Document::group_undo` joins the lines' steps into one `Batch` entry, since a later line reads features an earlier line made), and writes a command-log entry with the proposal, the user's prompt and the model id. Features it makes carry the assistant mark in the tree. If any line fails, the part goes back to how it was and nothing is left on the undo stack. The card's Undo button undoes the proposal while its step is the last one; the Edit menu's Undo works as for any other change, and the card then reads as undone.

The assistant can only propose. Its `apply` tool fails unless the user turned on Auto-apply (Assistant tab, Settings) for the session.

## Tools

The assistant gets `model_summary`, `feature`, `selection`, `measure`, `mass_properties`, `propose` and `apply`. The verbs are `sketch`, `extrude`, `cut`, `hole`, `linear_pattern`, `circular_pattern`, `mirror`, `draft`, `fillet`, `chamfer`, `edit_extrude`, `edit_cut`, `edit_hole`, `edit_pattern`, `edit_mirror`, `edit_draft`, `edit_fillet`, `edit_chamfer`, `suppress` and `delete_feature`; `extrude` and `cut` also take an up-to-next or up-to-face end; for the assistant they are the line types inside `propose`. Every tool's name, description and parameters come from the command registry in `commands.rs`, the table the ribbon, the keyboard shortcuts and `soliddraft commands` also read, so a new verb shows up everywhere at once. The parameter schemas sit beside the typed recipe lines in `command_tools.rs`.

`feature` lists the faces and edges a feature made, with positions, planar faces' normals and sketch frames, the two faces each edge joins, and whether the edge is a convex outside edge or a concave inside corner. That is how the model finds "the inside corner" without a selection. For a sketch it also numbers the dimensions.

`mass_properties` calculates a named feature's volume, area and centre of mass only on
request. Normal editing, proposal previews, selection descriptions and `model_summary`
read only previously requested results. Unmeasured summary and proposal quantities are
JSON `null`, not zero; the property panel omits those rows until Evaluate › Mass props
is requested. New bodies start unmeasured, while undo can reuse an earlier body's result.

An MCP client gets the same tools, plus each verb as a direct tool, and `rollback`, `undo`, `redo`, `rename`, `rename_plane`, `edit_dimension`, `save` and `open`. Modelling verbs run as one-line proposals, one undo step. Explicit MCP `suppress` and `delete_feature` commands follow the ribbon directly, so a missing downstream reference raises the repair prompt instead of rejecting the edit on a scratch copy. Assistant proposals still reject edits that break previously built features. `rename_plane` changes a datum's display label; `sketch` accepts its current label or its stable `top`, `front` or `right` identity. `edit_dimension` changes a sketch dimension by its number in `feature`. `save` writes the part to a `.dwprt` path without backups or a recent-file entry, and `open` replaces the session's part with a part file, or opens a drawing file with its part.

Every other command the ribbon runs is an MCP tool too, through `AppTools` in `app_tools.rs`: `new_part`, `save_as`, the four exports, `roll_to_end`, `make_drawing`, and the drawing's commands (`drawing_rebuild`, `drawing_section` with a cutting line through a model point, the view display and alignment commands, model items, centre marks, sheets, exports, `drawing_save`, `drawing_delete`). A tool takes by name what the ribbon takes from the selection, and runs the typed command the app's action runs; `model_summary` lists the open drawing's sheets, views and annotations by name and id, with each view's count of exact curves and of mesh polylines; a drawing tool takes a view by its id, or by a label no other view has, and refuses a label several views share. Undo and redo work on the drawing while it is the active document, as in the app. A command that needs the window (the camera, the panels, dialogs, the interactive sketcher and drawing canvas tools, the in-app assistant's prompts) has no tool, and its registry row says why: `soliddraft commands --json` prints it as `app_only`, and each row's `undo` says what it does to the undo stacks. `soliddraft run <command> [JSON]` and `soliddraft run --script FILE` run the same tools from a shell or a script; `crates/soliddraft-cli/tests/entry_points.rs` fails a command the app runs that has neither a tool nor a reason.

Entities are named as the interface names them: `Boss-Extrude1` for a feature, `Boss-Extrude1 · End cap` for a face, `Boss-Extrude2 · Edge 5` for an edge. Fillets and chamfers store the persistent reference the document makes for the named edge, so they follow it through rebuilds like any other fillet.

## The API request

Each request follows the current Messages API rules for `claude-opus-5-5`:

- No `thinking` field. Thinking is always on for this model, and `{type: "disabled"}` or a budget returns a 400. Effort is the control: `output_config.effort` is `medium`, the model's default, set explicitly.
- Every tool has `strict: true` and a schema in the strict subset: `additionalProperties: false` on every object, and no `minimum`, `maxLength` or similar keywords, which the API rejects. Bounds such as "radius greater than 0" are checked by the app and stated in the parameter descriptions. No length or coordinate in a command may exceed `MAX_LENGTH_MM` (100,000 mm): the kernel meshes every result, and its time and memory grow with the size of the shape, so a larger size is refused by name before it reaches the kernel. The server says so in its instructions.
- No `tool_choice`. Forced tool use returns a 400 on this model; the system prompt steers the model to `propose`.
- `eager_input_streaming: true` on every tool, so long proposals stream. The server then no longer checks the input, so the app parses each tool input strictly and validates it against the schema before running anything. Input that is not valid JSON goes back to the model as an `INVALID_JSON` error result.
- `fallbacks: "default"` with the beta header `server-side-fallback-2026-07-01`, so a safety refusal is retried on another model by the API instead of failing the chat. The app sends it only to `api.anthropic.com`, not to proxies set through `ANTHROPIC_BASE_URL`.
- Prompt caching: a breakpoint on the system prompt caches the tool list and the system prompt together (tools render first), and the top-level automatic breakpoint caches the conversation so far. The part summary goes into each user message, after the cached prefix, as a `<model_summary>` block.
- `max_tokens` is 64,000, since thinking counts toward it.

The history only grows. Thinking blocks go back to the API exactly as they came, with their signatures, which the model requires. A turn that fails, is stopped or is refused is removed whole, so no earlier message ever changes. After a mid-reply fallback, only text blocks from before the fallback marker go back, as the API documents.

A refusal (`stop_reason: "refusal"`) runs no tool calls, since a refusal can cut a tool call off mid-input. A reply cut off at `max_tokens` with a tool call in it does not run that call; the model gets an error result saying so.

## Providers

The assistant runs on the Anthropic API or on OpenRouter. Both take the same Messages API request, so tool use, proposals and MCP work the same on either.

OpenRouter is reached through its Anthropic-compatible endpoint, `POST https://openrouter.ai/api/v1/messages`, rather than its OpenAI-compatible chat completions. That endpoint streams the same events as the Anthropic API, passes thinking blocks with their signatures and native tool use through unchanged, and accepts `strict` tools, `eager_input_streaming`, `output_config.effort` and `cache_control`. The app's one stream reader and history rules therefore serve both providers; chat completions would have needed a second translation for tool calls and thinking. The OpenRouter request differs in four ways:

- The key goes in `Authorization: Bearer`, not `x-api-key`.
- Model ids carry the vendor: `anthropic/claude-opus-5.5`, not `claude-opus-5-5`.
- `thinking: {type: "adaptive", display: "omitted"}`, since OpenRouter asks for summarized thinking unless told otherwise and the app shows none.
- `provider: {order: ["anthropic"]}`, so requests go to Anthropic's own endpoint first, as OpenRouter advises for Claude clients. No server-side `fallbacks` or beta header: OpenRouter's `fallbacks` field means something else.

Which provider: `SOLIDDRAFT_PROVIDER` (`openrouter` or `anthropic`) first, then the Provider setting (Assistant tab, Settings group), then whichever provider has a key, Anthropic if both do. A provider chosen by name does not fall back to the other one; the chat panel says its key is missing. Which model: `SOLIDDRAFT_MODEL`, then the model set for that provider, then its default: `claude-opus-5-5` on Anthropic and `anthropic/claude-opus-5.5` on OpenRouter, the same model as on Anthropic. A model saved in the settings stays as saved. The assistant settings dialog saves both settings to `settings.json` in the config directory (`~/.config/soliddraft` on Linux) and reconnects; the new conversation starts fresh, and the messages, proposals and MCP endpoint stay.

## API keys

Each provider has its own key. The app reads it from `OPENROUTER_API_KEY` or `ANTHROPIC_API_KEY`, then from the OS keychain (Secret Service on Linux, Keychain on macOS, Credential Manager on Windows) under service `soliddraft`, user `openrouter-api-key` or `anthropic-api-key`.

Existing Draftwell settings still work. When a SolidDraft config file is absent, the app
reads its counterpart in the old `draftwell` config directory. New settings go into
`soliddraft`; the first save keeps the other sections from the old settings file. Recent
files and the personal material library use the same fallback. An explicit
`SOLIDDRAFT_CONFIG_DIR` isolates the config directory; `DRAFTWELL_CONFIG_DIR` still works
when the new variable is absent.

The other `DRAFTWELL_*` runtime variables remain aliases for the matching
`SOLIDDRAFT_*` variables. A present new variable takes precedence, even when empty.
If a provider has no `soliddraft` keychain entry, the app reads its old `draftwell`
entry. Saving a key writes only `soliddraft`; deleting it removes both entries.

An app started without environment variables, as a packaged release will be, takes its keys in the app: Provider on the Assistant tab opens the assistant settings, with one key row per provider beside the provider picker. Saving a key stores it in the OS keychain at once; the row then shows only its last four characters, with Replace and Delete. A key from the environment shows the same way and says it overrides the keychain. Save and reconnect starts using the change.

From the command line:

```sh
printf %s "$KEY" | soliddraft key set openrouter
soliddraft key status
```

`soliddraft key status` says where each key comes from and which provider and model the assistant uses, without printing a key. `soliddraft key delete openrouter` removes an entry.

A key never appears in logs: its type prints as `<redacted>`, and the request header is marked sensitive. Without a key the app runs, and the chat panel says the assistant is off and how to turn it on with either provider. `--no-assistant` keeps it off even when a key exists.

## MCP

The running app serves MCP with `--mcp [PORT]` (default 47811), on loopback only. It rejects requests whose `Origin` is not local, so a web page cannot drive the app:

```sh
cargo run --release -p soliddraft-app -- --demo --mcp
claude mcp add --transport http soliddraft http://127.0.0.1:47811/mcp
```

The headless command line serves a new part over stdio, or over HTTP with `--http PORT`:

```sh
claude mcp add soliddraft -- soliddraft mcp --demo
```

## Tests

The assistant tests run without a network or a key. `MockProvider` encodes scripted replies as real event streams, with empty thinking blocks and signatures as `claude-opus-5-5` sends them, and reads them back through the same parser as the real provider. The phase 3 exit test, `crates/soliddraft-app/tests/assistant_exit_test.rs`, builds the sample bracket without its fillet, hole pattern and slot, then runs the three example exchanges from text: "Add a fillet to the inside corner, R8", "Add four 9 mm holes on a 90 x 44 pattern" and "Cut a 40 x 12 slot through the flange". Each proposal is previewed, applied (the test checks the volume each one adds or removes, and the command log) and undone. An MCP client then makes the same three changes over HTTP and undoes them. `crates/soliddraft-cli/tests/mcp_stdio.rs` drives the command line over stdio, and the interface test in `exit_test.rs` clicks a proposal card through Preview, Apply and Undo.

The exit test also runs against the real model when a key is available, on the provider the app would choose, and prints that it skipped that part otherwise. `SOLIDDRAFT_RECORD_DIR=dir` saves every real stream to `dir` as a new fixture.
