import { useCallback, useState } from 'react'

/** Loading and error state around one user-triggered async action (a search,
 * filter, diff or coverage run): `run(action)` clears the error, sets
 * `loading` while `action` runs, and keeps a thrown error's message. */
export function useAsyncAction() {
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = useCallback(async (action: () => Promise<void>) => {
    setLoading(true)
    setError(null)
    try {
      await action()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  return { loading, error, run }
}
