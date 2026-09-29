# Third-party notices

## Mr. Mak Workspace

Workbench's Workspace feature (deliverable cards, sandboxed report serving, the 3D
compare viewer and the agents' authoring guide) is adapted from
[Mr. Mak Workspace](https://github.com/witnesstodark/mr-mak-workspace). The adapted
parts are in:

- `server/src/workspace/mod.rs`, `model.rs`, `view.rs`, `tools.rs`
- `server/src/workspace/assets/report.css`, `report.js`
- `web/src/features/workspace/index.ts`, `logic.ts`
- `web/src/features/workspace/compare3d/Compare3D.tsx`, `viewer.ts`

Mr. Mak Workspace is distributed under the MIT License:

```
MIT License

Copyright (c) 2026 Mr. Mak Workspace contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## ConPTY (Windows release archive)

The Windows release archive ships `conpty.dll` and `OpenConsole.exe` unmodified, the
x64 files of Microsoft's
[Microsoft.Windows.Console.ConPTY](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY)
package (the version is pinned in `.github/workflows/release.yml`), built from
[Windows Terminal](https://github.com/microsoft/terminal). Workbench's terminals use them
in place of the console host built into Windows. Microsoft lists the third-party components
of that repository in its
[NOTICE.md](https://github.com/microsoft/terminal/blob/main/NOTICE.md).

The package is distributed under the MIT License:

```
Copyright (c) Microsoft Corporation. All rights reserved.

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED *AS IS*, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Dependencies (Rust crates in `server/Cargo.toml`, npm packages in `web/package.json`)
carry their own licenses in their published packages.
