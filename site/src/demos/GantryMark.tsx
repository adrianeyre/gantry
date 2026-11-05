// Gantry's own mark (web/public/favicon.svg), drawn inline so it scales with
// the stage. The gradient id is per instance: several marks share a page.
import { useId } from 'react'

export function GantryMark({ size = 20 }: { size?: number }) {
  const id = useId()
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" aria-hidden="true" style={{ display: 'block', flex: 'none' }}>
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#1d4ed8" />
          <stop offset="1" stopColor="#13338c" />
        </linearGradient>
      </defs>
      <rect width="64" height="64" rx="14" fill={`url(#${id})`} />
      <g fill="none" stroke="#fff" strokeLinecap="round" strokeLinejoin="round">
        <path d="M14 50V16h36v34" strokeWidth="5" />
        <rect x="25" y="12" width="14" height="13" rx="3" fill="#fff" stroke="none" />
        <path d="M32 25v7" strokeWidth="4" />
        <path d="M21 44h22" strokeWidth="5" />
      </g>
    </svg>
  )
}
