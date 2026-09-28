// The internals of monaco-editor that lib/monacoSetup.ts reaches into (the
// package ships no declarations for its deep ESM modules).
declare module 'monaco-editor/platform/actions/common/actions' {
  export interface MenuItemLike {
    command?: { id: string }
    submenu?: unknown
  }
  export const MenuId: { EditorContext: unknown; EditorContextPeek: unknown }
  export const MenuRegistry: { getMenuItems(id: unknown): MenuItemLike[] }
}

declare module 'monaco-editor/base/browser/ui/contextview/contextview' {
  export class ContextView {
    setContainer(container: HTMLElement | null, domPosition: number): void
  }
}
