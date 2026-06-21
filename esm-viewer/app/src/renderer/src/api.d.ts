import type { Fo76Api } from '../../shared/api-types'

declare global {
  interface Window {
    api: Fo76Api
  }
}
