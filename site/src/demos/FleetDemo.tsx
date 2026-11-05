import type { ReactNode } from 'react'
import { Bell } from 'lucide-react'
import { Badge, Bar, Button, Collapse, Count, Meter, Roll, Stage, Window, type Act, type Layout } from '../kit'
import { GantryMark } from './GantryMark'
import { useScheme } from './scheme'
import './fleet.css'

// The homepage hero: Gantry's Fleet page, in a browser, as a wide hero.
// The story is the one Gantry is built around: a Job finishes, the Printer
// waits in Awaiting Clear and an Alert says so; someone marks the bed clear
// and the next Job in the Queue starts on it.
//
// Real strings: the page's title, the three Printers' names and states, and
// on the Voron (the card the story happens on) its Job, layer, time left and
// button. Everything else is placeholder lines around Gantry's own anatomy:
// the nav, the Connected badges, progress, filament slots.

type Phase = 'printing' | 'done' | 'cleared' | 'next'

type S = {
  phase: Phase
  progress: number
  layer: number
  alert: boolean
}

const initial: S = { phase: 'printing', progress: 96, layer: 238, alert: false }

const JOBS = { first: 'benchy.gcode.3mf', next: 'bracket-v3.gcode.3mf' }
const LAYERS = { first: 248, next: 312 }

/** The finished picture: the next Job running on the Printer just cleared. */
const POSTER: S = { phase: 'next', progress: 2, layer: 6, alert: false }

const state = (s: S) =>
  s.phase === 'done' || s.phase === 'cleared' ? 'Awaiting Clear' : `Printing · ${s.progress}%`

async function script(a: Act<S>) {
  await a.wait(900)

  // 1. The last layers print, and the Job finishes
  a.mark('finish')
  await a.set({ progress: 98, layer: 243 })
  await a.wait(900)
  await a.set({ progress: 100, layer: 248 })
  await a.wait(700)
  await a.set({ phase: 'done' })
  await a.wait(500)

  // 2. The Alert arrives
  a.mark('alert')
  await a.set({ alert: true })
  await a.wait(1600)

  // 3. Someone takes the part off and marks the bed clear
  a.mark('clear')
  await a.move({ at: 'mark-clear', x: 0.84, y: 0.62 })
  await a.wait(250)
  await a.click()
  await a.set({ phase: 'cleared', alert: false, progress: 0 })
  await a.wait(900)

  // 4. The Queue's next Job starts on it
  a.mark('next')
  await a.set({ phase: 'next', layer: 1 })
  await a.move({ at: 'rest' })
  await a.wait(1100)
  await a.set({ progress: 1, layer: 3 })
  await a.wait(1100)
  await a.set({ progress: 2, layer: 6 })
  await a.wait(2400)
}

export function FleetDemo() {
  const scheme = useScheme()
  return (
    <Stage
      width={1040}
      height={500}
      compact={{ below: 720, width: 358, height: 340 }}
      initial={initial}
      poster={POSTER}
      script={script}
      play="once"
      theme={scheme}
      cursor={{ at: { wide: { x: 760, y: 470 }, compact: { x: 300, y: 320 } } }}
      className="gantry-look fleet-demo"
    >
      {(s, _rect, layout) => (
        <>
          <Window url="gantry.local" urlShape="pill" className="fleet-window">
            {layout === 'wide' && <Nav />}
            <div className="fleet-page">
              <div className="fleet-head">
                <span className="fleet-title">Fleet</span>
                <Badge tone="success" dot>
                  Live
                </Badge>
              </div>
              <div className="fleet-grid">
                {layout === 'wide' && (
                  <PrinterCard name="Bambu X1C" state="Printing · 41%">
                    <Meter value={0.41} className="fleet-meter" />
                    <Filler />
                  </PrinterCard>
                )}
                <Voron s={s} layout={layout} />
                {layout === 'wide' && (
                  <PrinterCard name="Prusa MK4" state="Idle">
                    <Filler idle />
                  </PrinterCard>
                )}
              </div>
            </div>
          </Window>

          <Collapse open={s.alert} reveal="drop" className="fleet-alert-slot">
            <div className="fleet-alert">
              <span className="fleet-alert__icon">
                <Bell size={16} strokeWidth={2} />
              </span>
              <div className="fleet-alert__words">
                <span className="fleet-alert__title">Voron 2.4 is Awaiting Clear</span>
                <span className="fleet-alert__body">{JOBS.first} finished in 2 h 14 min</span>
              </div>
            </div>
          </Collapse>

          <span data-target="rest" className="fleet-rest" />
        </>
      )}
    </Stage>
  )
}

function Nav() {
  return (
    <div className="fleet-nav">
      <span className="fleet-brand">
        <GantryMark size={18} />
        <span>Gantry</span>
      </span>
      <span className="fleet-tab" data-on>
        Fleet
      </span>
      {[46, 40, 38, 48, 66, 54].map((w, i) => (
        <span key={i} className="fleet-tab">
          <Bar w={w} h={6} tone={2} />
        </span>
      ))}
      <span className="fleet-nav__end">
        <Bar w={56} h={24} r={7} tone={1} />
      </span>
    </div>
  )
}

function PrinterCard({ name, state, aside, children, ...rest }: { name: string; state: ReactNode; aside?: ReactNode; children: ReactNode } & Record<`data-${string}`, unknown>) {
  return (
    <div className="fleet-card" {...rest}>
      <div className="fleet-card__top">
        <div className="fleet-card__head">
          <span className="fleet-card__name">{name}</span>
          <Badge tone="success" dot>
            Connected
          </Badge>
        </div>
        <div className="fleet-card__state">
          <span>{state}</span>
          {aside}
        </div>
      </div>
      {children}
    </div>
  )
}

/** Placeholder rows for a card the story doesn't touch, at a real card's pitch. */
function Filler({ idle }: { idle?: boolean }) {
  const rows = idle ? [[44, 30], [30, 28], [58, 22], [70, 18], [48, 26], [62, 20], [40, 24]] : [[30, 96], [52, 70], [40, 54], [88, 44]]
  return (
    <>
      {!idle && (
        <div className="fleet-job">
          <Bar w={44} h={44} r={8} tone={1} />
          <div className="fleet-rows">
            {rows.slice(0, 2).map(([k, v], i) => (
              <div key={i} className="fleet-row">
                <Bar w={k} h={6} tone={2} />
                <Bar w={v} h={6} tone={3} />
              </div>
            ))}
          </div>
        </div>
      )}
      <div className="fleet-rows">
        {(idle ? rows : rows.slice(2)).map(([k, v], i) => (
          <div key={i} className="fleet-row">
            <Bar w={k} h={6} tone={2} />
            <Bar w={v} h={6} tone={3} />
          </div>
        ))}
      </div>
      <div className="fleet-slots">
        {[52, 58, 50].map((w, i) => (
          <Bar key={i} w={w} h={20} r={10} tone={1} />
        ))}
      </div>
    </>
  )
}

function Voron({ s, layout }: { s: S; layout: Layout }) {
  const next = s.phase === 'next'
  const waiting = s.phase === 'done' || s.phase === 'cleared'
  const job = next ? JOBS.next : JOBS.first
  const layers = next ? LAYERS.next : LAYERS.first
  return (
    <PrinterCard
      name="Voron 2.4"
      state={<Roll>{state(s)}</Roll>}
      aside={
        <span className="fleet-note" data-show={next || undefined}>
          From the Queue
        </span>
      }
      data-target="voron"
    >
      <Meter value={s.progress / 100} tone={s.phase === 'done' ? 'success' : 'accent'} className="fleet-meter" />
      <div className="fleet-job">
        <span className="fleet-thumb" data-next={next || undefined} />
        <div className="fleet-rows">
          <div className="fleet-row">
            <span className="fleet-key">Print File</span>
            <Roll className="fleet-value">{job}</Roll>
          </div>
          <div className="fleet-row">
            <span className="fleet-key">Layer</span>
            <span className="fleet-value fleet-layer">
              <Count value={s.layer} />
              <Roll>{`of ${layers}`}</Roll>
            </span>
          </div>
        </div>
      </div>
      {layout === 'wide' && (
        <div className="fleet-rows">
          <div className="fleet-row">
            <span className="fleet-key">Time remaining</span>
            <Roll className="fleet-value">{waiting ? 'Done' : next ? '1 h 52 min' : s.progress >= 98 ? '1 min' : '4 min'}</Roll>
          </div>
          <div className="fleet-row">
            <span className="fleet-key">Nozzle</span>
            <Roll className="fleet-value">{waiting ? '220 °C → 0 °C' : '220 °C → 220 °C'}</Roll>
          </div>
        </div>
      )}
      <div className="fleet-actions">
        <Button variant={s.phase === 'done' ? 'primary' : 'secondary'} data-target="mark-clear">
          {s.phase === 'done' ? 'Mark Clear' : s.phase === 'cleared' ? 'Cleared' : 'Pause'}
        </Button>
        <Bar w={84} h={28} r={8} tone={1} />
      </div>
    </PrinterCard>
  )
}
