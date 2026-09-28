import { useCallback, useRef, useState } from 'react'

/** Loading and error state around one user-triggered async action (a search,
 * filter, diff or coverage run): `run(action)` clears the error, sets
 * `loading` while `action` runs, and keeps a thrown error's message. A `run`
 * while one is still in flight is ignored. */
export function useAsyncAction() {
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const running = useRef(false)

  const run = useCallback(async (action: () => Promise<void>) => {
    if (running.current) return
    running.current = true
    setLoading(true)
    setError(null)
    try {
      await action()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      running.current = false
      setLoading(false)
    }
  }, [])

  return { loading, error, run }
}
