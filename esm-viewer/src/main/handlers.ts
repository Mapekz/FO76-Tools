import type { OpenDialogOptions } from 'electron'
import { CH, type DbHandle, type DbId } from '../shared/api-types'
import type { EsmHost } from './addon'
import type { DbRegistry } from './db-registry'
import { validateDiffRequest, validateOp } from './ipc-validators'

export interface HandlerDeps {
  host: EsmHost
  registry: DbRegistry
  parseFormId(s: string): string
  showOpenDialog(options: OpenDialogOptions): Promise<{ canceled: boolean; filePaths: string[] }>
}

type Handler = (...args: unknown[]) => unknown

/** Every IPC handler, keyed by channel, over injected dependencies —
 * `registerIpc` wires them to Electron and tests call them directly. */
export function createHandlers(deps: HandlerDeps): Record<string, Handler> {
  const { host, registry } = deps

  function entry(id: unknown): DbHandle {
    const handle = typeof id === 'string' ? registry.get(id) : undefined
    if (!handle) throw new Error(`no database with id ${String(id)}`)
    return handle
  }

  async function pick(options: OpenDialogOptions): Promise<string | null> {
    const { canceled, filePaths } = await deps.showOpenDialog(options)
    return canceled ? null : (filePaths[0] ?? null)
  }

  return {
    [CH.openFileDialog]: () =>
      pick({ filters: [{ name: 'ESM Files', extensions: ['esm'] }], properties: ['openFile'] }),
    [CH.openFolderDialog]: () => pick({ properties: ['openDirectory'] }),
    [CH.openDatabase]: async (path) => {
      if (typeof path !== 'string' || path.length === 0) throw new Error('invalid path')
      const info = await host.open(path)
      return registry.add(path, info)
    },
    [CH.closeDatabase]: (id) => {
      const closed = registry.remove(id as DbId)
      if (closed && !registry.isOpen(closed.info.path)) host.close(closed.info.path)
    },
    [CH.listOpen]: () => registry.listAll(),
    [CH.parseFormId]: (s) => {
      if (typeof s !== 'string') throw new Error('invalid FormID')
      return deps.parseFormId(s)
    },
    // The host keys each open file by its canonical path (`info.path`).
    [CH.run]: (id, op) => host.run(entry(id).info.path, validateOp(op)),
    [CH.diff]: (oldId, newId, request) => {
      const { record_type, options } = validateDiffRequest(request)
      return host.run(entry(oldId).info.path, {
        op: 'diff',
        b: entry(newId).info.path,
        record_type,
        // Omitted fields take the engine's defaults (`DiffOptions` is
        // `#[serde(default)]`), which the generated type can't express.
        options: options as Required<typeof options>,
      })
    },
  }
}
