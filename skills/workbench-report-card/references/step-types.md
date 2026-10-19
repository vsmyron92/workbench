# Step types, limits and the 3D manifest

## How a step's viewer is chosen

`workspace_add_step` takes an optional `viewer`: `auto` (the default), `html`, `markdown`, `image`, `gallery`, `compare3d`, `pdf`, `video`, `audio` or `text`. With `auto` the path decides:

| Path | Viewer |
| --- | --- |
| a folder | gallery |
| `.html`, `.htm` | HTML report in the sandbox |
| `.md`, `.markdown` | Markdown |
| `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.svg`, `.bmp`, `.avif`, `.ico` | image |
| `.pdf` | PDF |
| `.mp4`, `.webm`, `.mov`, `.m4v`, `.ogv` | video |
| `.mp3`, `.wav`, `.ogg`, `.oga`, `.m4a`, `.flac`, `.aac`, `.opus` | audio |
| `.glb`, `.gltf` | 3D viewer with that one model |
| `.txt`, `.log`, `.json`, `.jsonl`, `.csv`, `.tsv`, `.yaml`, `.yml`, `.toml`, `.xml`, `.ini`, `.cfg`, `.conf`, source files such as `.rs`, `.ts`, `.js`, `.py`, `.sh`, `.sql`, `.diff`, `.c`, `.cpp`, `.go`, `.java` | text |
| anything else, such as `.zip`, `.xlsx`, `.docx` | no preview, only a download button |

A 3D comparison manifest is a `.json` file, so register it with `viewer: "compare3d"`; with `auto` it would show as text.

## What each viewer does

- **HTML report**: a sandboxed frame, reloaded when the file changes. See `html-skeleton.md` and the sandbox rules in `SKILL.md`.
- **Markdown**: GitHub-flavoured, with tables, task lists, footnotes, alerts (`> [!NOTE]`), code blocks and Mermaid diagrams (a fenced block with the language `mermaid`). Raw HTML is sanitised: no scripts, styles or iframes. Relative images and links resolve inside the card folder (`![alt](img/chart.png)`); a link to another file of the card opens it (as that step when it is one, else in a new tab), `#anchor` links scroll, links that leave the card folder only show a notice. The user can edit a Markdown step in the card and save it, so read the file again before you overwrite it.
- **Image**: one picture, fit or 100%, with the lightbox.
- **Gallery**: a folder shown as a grid; click opens the lightbox with arrow keys. The file name is the caption and files come in natural order (`img2` before `img10`), so name them `01-baseline.png`, `02-with-cache.png`. Subfolders can be browsed, so one gallery step can hold several rounds (`round-1/`, `round-2/`); videos in the folder play too. A folder lists 2000 entries per page.
- **PDF**: the browser's own PDF viewer.
- **Video and audio**: native players; seeking works.
- **Text**: monospace; a valid `.json` under 1 MB is pretty-printed.
- **File**: no preview; the user can download it. Say in the card's description what it is.

Markdown and text steps larger than 2 MB are not shown ("open it raw instead"): split them.

## 3D comparison manifest

A `compare3d` step opens a JSON manifest. Each test shows its models side by side, three panes to a row, with cameras locked together and one shading toolbar. Every model is scaled into the same box, so shapes compare regardless of the exporter's units; triangle, vertex and mesh counts and the texture size appear on each pane. Models are `.glb` or `.gltf`; Draco compression works, meshopt compression does not. All paths are relative to the manifest file.

```ts
interface Manifest {
  title?: string;            // shown when there is a single test
  tests: Test[];             // at least one test with at least one model
}
interface Test {
  id: string;                // unique: the chosen shading mode is remembered per id
  name: string;              // the test picker's label
  kind?: "lowpoly" | "highpoly"; // lowpoly opens in wireframe, otherwise textured
  defaultMode?: "wire" | "solid" | "normals" | "pbr" | "albedo" | "normalMap" | "rough" | "metal";
  note?: string;             // shown under the toolbar when there are several tests
  input?: string;            // a reference image, shown on request
  models: Model[];
}
interface Model {
  file: string;              // "models/a.glb"
  label: string;             // the pane's name (the file name when missing)
  note?: string;             // small text beside the name
  accent?: string;           // a CSS colour for the name
  rotationY?: number;        // degrees, for exports that face the wrong way
  alias?: string;            // a cover name shown instead of the label until the user reveals it
  logo?: string;             // a small image beside the name
}
```

`defaultMode` overrides `kind`. A model with an `alias` shows it instead of its `label` until the user clicks the eye button on its pane; use it for blind comparisons and give the key in your answer. Example:

```json
{
  "title": "Wheel exporters",
  "tests": [
    {
      "id": "wheel",
      "name": "Wheel",
      "kind": "lowpoly",
      "note": "Same source mesh, two exporters",
      "input": "refs/wheel.png",
      "models": [
        { "file": "models/wheel-a.glb", "label": "Exporter A", "note": "4k triangles", "alias": "Model 1" },
        { "file": "models/wheel-b.glb", "label": "Exporter B", "rotationY": 180, "alias": "Model 2" }
      ]
    }
  ]
}
```

Register it with `workspace_add_step` (`path: "compare.json"`, `viewer: "compare3d"`). A manifest without tests, or whose tests list no model file, shows an error in the tab.

## Limits

| What | Limit or rule |
| --- | --- |
| Card title | 200 characters, required |
| Description | 4000 characters |
| Category | 40 characters; lower-cased, spaces become hyphens |
| Step name | 120 characters |
| `workspace_write_file` | 20 MB per call; replaces an existing file; creates folders as needed |
| Markdown or text step shown | 2 MB |
| A project file or folder added as a step | copied into the card folder under a free name (`name (2).ext`) at its top level; a folder copies up to 2000 files and 2 GB, without private names (see below) and symbolic links |
| Card id | the title as a lower-case slug (60 characters), then `-2`, `-3` when taken |
| Auto-archive | 7 days without a change, unless pinned |
| Statuses | `active`, `done`, `archived` |

Never served and never accepted as a step or a written file: any name or folder starting with a dot, `node_modules`, `auth.json`, `token.json`, `tokens.json`, `oauth_*.json`, `*secret*.json`, `credentials*.json`, names ending `.pem`, `.key`, `.p12`, `.pfx`, `_token`, `.token`, `_api_key` or `.api_key`, and names starting `id_rsa`, `id_ecdsa`, `id_ed25519` or `id_dsa`. Rename the file or keep it out of the card.
