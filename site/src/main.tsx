// Mounts the Stageshow demos into the static page. Each `[data-demo]` holds a
// screenshot or a still as its fallback; the demo replaces it as it nears the
// viewport, so a reader without JavaScript still sees the product.
import { useState, type ComponentType } from 'react'
import { createRoot } from 'react-dom/client'
import './demos/look.css'
import { enhance } from './page'

enhance()

const demos: Record<string, () => Promise<ComponentType>> = {
  fleet: () => import('./demos/FleetDemo').then((m) => m.FleetDemo),
  queue: () => import('./demos/QueueDemo').then((m) => m.QueueDemo),
  alerts: () => import('./demos/AlertsDemo').then((m) => m.AlertsDemo),
  history: () => import('./demos/HistoryDemo').then((m) => m.HistoryDemo),
  terminal: () => import('./demos/TerminalDemo').then((m) => m.TerminalDemo),
}

/** A demo that plays once, with a Replay button that remounts it (Stageshow's explicit replay). */
function Replayable({ Demo }: { Demo: ComponentType }) {
  const [run, setRun] = useState(0)
  return (
    <>
      <Demo key={run} />
      <button className="replay" type="button" onClick={() => setRun((n) => n + 1)}>
        <svg className="icon" aria-hidden="true" viewBox="0 0 24 24">
          <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
          <path d="M3 3v5h5" />
        </svg>
        Replay
      </button>
    </>
  )
}

async function mount(slot: HTMLElement) {
  const load = demos[slot.dataset.demo ?? '']
  if (!load) return
  const Demo = await load()
  slot.classList.add('mounted')
  createRoot(slot).render(slot.hasAttribute('data-replay') ? <Replayable Demo={Demo} /> : <Demo />)
}

const nearby = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue
      nearby.unobserve(entry.target)
      void mount(entry.target as HTMLElement)
    }
  },
  { rootMargin: '400px 0px' },
)
// `?mount=all` mounts every demo at once, for filming them (Stageshow's film.mjs doesn't scroll)
const all = new URLSearchParams(location.search).get('mount') === 'all'
for (const slot of document.querySelectorAll<HTMLElement>('[data-demo]')) {
  if (all) void mount(slot)
  else nearby.observe(slot)
}
