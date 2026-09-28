/** Apply `request`'s result (or report its error) unless the returned cancel
 * ran first. An effect returns the cancel as its cleanup, so the response to
 * an input that has since changed (another file, another type) never lands. */
export function applyUnlessCancelled<T>(
  request: Promise<T>,
  apply: (value: T) => void,
  onError: (e: unknown) => void = console.error,
): () => void {
  let cancelled = false
  request.then(
    (value) => {
      if (!cancelled) apply(value)
    },
    (e: unknown) => {
      if (!cancelled) onError(e)
    },
  )
  return () => {
    cancelled = true
  }
}
