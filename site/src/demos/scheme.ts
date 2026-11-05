import { useSyncExternalStore } from 'react'

// The page's colour scheme, so every stage is drawn light or dark with it.
const query = typeof window !== 'undefined' ? window.matchMedia('(prefers-color-scheme: dark)') : null

function subscribe(change: () => void) {
  query?.addEventListener('change', change)
  return () => query?.removeEventListener('change', change)
}

export function useScheme(): 'light' | 'dark' {
  return useSyncExternalStore(subscribe, () => (query?.matches ? 'dark' : 'light'), () => 'light')
}
