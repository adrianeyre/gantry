import { Collapse, LockScreen, Notification, Phone, Stage, WALLPAPER, type Act } from '../kit'
import { GantryMark } from './GantryMark'
import './tiles.css'

// The Alerts tile (tall): a phone pinned a gutter from the top and run off
// the bottom, on its lock screen. One beat as it scrolls in: a Fault's Alert
// arrives above an older one. The phone stays light and lit throughout; the
// tile's own ground is the page's.

type S = { fault: boolean }

async function script(a: Act<S>) {
  await a.wait(800)
  await a.set({ fault: true })
  await a.wait(2500)
}

export function AlertsDemo() {
  return (
    <Stage
      width={372}
      height={440}
      initial={{ fault: false }}
      poster={{ fault: true }}
      script={script}
      play="once"
      className="gantry-look tile-stage alerts-demo"
    >
      {(s) => (
        <Phone width={300} frame="pro" dark wallpaper={WALLPAPER} className="alerts-phone">
          <LockScreen stack="top" date="Thursday, 5 November" time="21:47">
            <Collapse open={s.fault} reveal="drop">
              <Notification
                icon={<GantryMark size={38} />}
                title="Prusa MK4 has a Fault"
                body="Heater failure on the nozzle. Its Job is paused."
              />
            </Collapse>
            <Notification icon={<GantryMark size={38} />} title="Filament low on Ender 3 V3" body="PLA in slot 1 is at 8%." time="12m ago" />
          </LockScreen>
        </Phone>
      )}
    </Stage>
  )
}
