import { Badge, Bar, Collapse, Stage, type Act } from '../kit'
import { useScheme } from './scheme'
import './tiles.css'

// The Queue tile (the grid's hero, two columns): the Queue's list, no
// window chrome. One beat as it scrolls in: a Job queued for any Compatible
// Printer drops in under the Jobs already waiting, and the filler row below
// moves down with it. Real words: the three Jobs and who each waits for.

type S = { added: boolean }

type Tone = 'accent' | 'neutral' | 'success'
type Job = { file: string; who: string; status: string; tone: Tone; swatch: string }

const JOBS: Job[] = [
  { file: 'gridfinity-3x2.gcode.3mf', who: 'Bambu X1C', status: 'Printing', tone: 'accent', swatch: '#d9a521' },
  { file: 'cable-clips.gcode', who: 'Prusa MK4', status: 'Next', tone: 'neutral', swatch: '#3c6fd8' },
]
const ADDED: Job = { file: 'spool-holder.gcode.3mf', who: 'Any Compatible Printer', status: 'Waiting', tone: 'neutral', swatch: '#8a5cc7' }

async function script(a: Act<S>) {
  await a.wait(600)
  await a.set({ added: true })
  await a.wait(2500)
}

function JobRow({ job }: { job: Job }) {
  return (
    <div className="q-row">
      <span className="q-thumb" style={{ background: job.swatch }} />
      <div className="q-words">
        <span className="q-file">{job.file}</span>
        <span className="q-who">{job.who}</span>
      </div>
      <Badge tone={job.tone}>{job.status}</Badge>
    </div>
  )
}

export function QueueDemo() {
  const scheme = useScheme()
  return (
    <Stage
      width={760}
      height={260}
      compact={{ below: 560, width: 358, height: 260 }}
      initial={{ added: false }}
      poster={{ added: true }}
      script={script}
      play="once"
      theme={scheme}
      className="gantry-look tile-stage queue-demo"
    >
      {(s) => (
        <div className="q-card">
          <div className="q-head">
            <span className="q-title">Queue</span>
            <Bar w={72} h={26} r={7} tone={1} />
          </div>
          {JOBS.map((job) => (
            <JobRow key={job.file} job={job} />
          ))}
          <Collapse open={s.added} reveal="drop">
            <JobRow job={ADDED} />
          </Collapse>
          <div className="q-row">
            <Bar w={36} h={36} r={8} tone={1} />
            <div className="q-words q-words--bars">
              <Bar w={150} h={7} tone={2} />
              <Bar w={92} h={6} tone={1} />
            </div>
            <Bar w={60} h={20} r={10} tone={1} />
          </div>
          <div className="q-row">
            <Bar w={36} h={36} r={8} tone={1} />
            <div className="q-words q-words--bars">
              <Bar w={128} h={7} tone={2} />
              <Bar w={104} h={6} tone={1} />
            </div>
            <Bar w={52} h={20} r={10} tone={1} />
          </div>
        </div>
      )}
    </Stage>
  )
}
