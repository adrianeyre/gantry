import { Chart, Count, Stage, type Act } from '../kit'
import { useScheme } from './scheme'
import './tiles.css'

// The History tile: one number and the line it came from, no chrome. One
// beat as it scrolls in: the last 30 days draw in and the success rate
// rolls up to where they land.

const DAYS = [82, 86, 84, 88, 87, 90, 89, 91, 88, 92, 93, 91, 94, 92, 95, 94, 93, 95, 96, 94, 95, 97, 96, 95, 97, 96, 98, 97, 96, 97]

// Drawn from 75%, so a month of small changes reads as a line, not a flat bar
const LINE = DAYS.map((d) => d - 75)

type S = { shown: boolean }

async function script(a: Act<S>) {
  await a.wait(500)
  await a.set({ shown: true })
  await a.wait(2500)
}

export function HistoryDemo() {
  const scheme = useScheme()
  return (
    <Stage
      width={372}
      height={210}
      initial={{ shown: false }}
      poster={{ shown: true }}
      script={script}
      play="once"
      theme={scheme}
      className="gantry-look tile-stage history-demo"
    >
      {(s) => (
        <div className="h-card">
          <span className="h-label">Success rate, last 30 days</span>
          <span className="h-figure">
            <Count value={s.shown ? 96.8 : 82.0} format={(n) => `${n.toFixed(1)}%`} duration={1100} />
          </span>
          <Chart data={LINE} width={282} height={84} show={s.shown} className="h-chart" />
        </div>
      )}
    </Stage>
  )
}
