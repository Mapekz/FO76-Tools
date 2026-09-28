import { dialog, ipcMain } from 'electron'
import { createHost, parseFormId } from './addon'
import { DbRegistry } from './db-registry'
import { createHandlers } from './handlers'

export function registerIpc(): void {
  const handlers = createHandlers({
    host: createHost(),
    registry: new DbRegistry(),
    parseFormId,
    showOpenDialog: (options) => dialog.showOpenDialog(options),
  })
  for (const [channel, handler] of Object.entries(handlers)) {
    ipcMain.handle(channel, async (_event, ...args: unknown[]) => {
      try {
        return await handler(...args)
      } catch (e: unknown) {
        throw new Error(e instanceof Error ? e.message : String(e), { cause: e })
      }
    })
  }
}
