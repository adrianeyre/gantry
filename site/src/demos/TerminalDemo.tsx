import { Caret, Collapse, Prompt, Stage, Window, type Act } from '../kit'
import './tiles.css'

// Quick start, beside the commands: what running them looks like. A
// terminal, pinned a gutter in from the top and left and run off the right
// and bottom. Two commands are typed, Compose's and Gantry's, and each
// answers with the lines it really prints. Plays once, in view.

type S = { first: string; started: boolean; second: string; created: boolean }

const RUN = 'docker compose up -d'
const USER = 'docker compose exec gantry gantry user create --admin --username admin'

const initial: S = { first: '', started: false, second: '', created: false }
const POSTER: S = { first: RUN, started: true, second: USER, created: true }

async function script(a: Act<S>) {
  await a.wait(600)
  await a.type('first', RUN, { interval: 28 })
  await a.wait(500)
  await a.set({ started: true })
  await a.wait(1100)
  await a.type('second', USER, { interval: 28 })
  await a.wait(500)
  await a.set({ created: true })
  await a.wait(2500)
}

export function TerminalDemo() {
  return (
    <Stage
      width={496}
      height={360}
      compact={{ below: 460, width: 358, height: 300 }}
      initial={initial}
      poster={POSTER}
      script={script}
      play="once"
      theme="dark"
      className="gantry-look tile-stage terminal-demo"
    >
      {(s) => (
        <Window chrome="terminal" title="gantry — zsh" className="t-window">
          <div className="t-body">
            <Prompt path="~/gantry">
              {s.first}
              {!s.started && <Caret still={s.first.length < RUN.length} />}
            </Prompt>
            <Collapse open={s.started}>
              <div>
                <span className="ok">✔</span> Network gantry_default <span className="t-right ok">Created</span>
              </div>
              <div>
                <span className="ok">✔</span> Container gantry <span className="t-right ok">Started</span>
              </div>
            </Collapse>
            <Collapse open={s.started}>
              <Prompt path="~/gantry">
                {s.second}
                {!s.created && <Caret still={s.second.length > 0 && s.second.length < USER.length} />}
              </Prompt>
            </Collapse>
            <Collapse open={s.created}>
              <div className="dim">Password for admin:</div>
              <div className="dim">Repeat the password:</div>
              <div>Created the Admin account `admin`.</div>
            </Collapse>
          </div>
        </Window>
      )}
    </Stage>
  )
}
