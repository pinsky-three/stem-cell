import { useState, useEffect, useCallback, useMemo } from 'react'

interface GridDot {
  id: string
  x: number
  y: number
  delay: number
  opacity: number
}

const STYLE_ID = 'analysis-loader-keyframes'

function ensureKeyframes() {
  if (typeof document === 'undefined') return
  if (document.getElementById(STYLE_ID)) return
  const style = document.createElement('style')
  style.id = STYLE_ID
  style.textContent = `
    @keyframes analysis-blink {
      0%, 100% { opacity: 0.2; }
      50% { opacity: 1; }
    }
    @keyframes analysis-fade {
      to { opacity: 0; transform: scale(0.5); }
    }
  `
  document.head.appendChild(style)
}

// Future: set revealImage to true + provide src to enable ImageLoader reveal
interface AnalysisLoaderProps {
  revealImage?: boolean
}

export default function AnalysisLoader(_props: AnalysisLoaderProps) {
  const [active, setActive] = useState(false)
  const [status, setStatus] = useState("")
  const [done, setDone] = useState(false)
  const [runKey, setRunKey] = useState(0)

  useEffect(() => { ensureKeyframes() }, [])

  const handleStart = useCallback((e: Event) => {
    const detail = (e as CustomEvent).detail
    setActive(true)
    setDone(false)
    setStatus(detail?.message || "Analyzing...")
    setRunKey(k => k + 1)
  }, [])

  const handleProgress = useCallback((e: Event) => {
    const detail = (e as CustomEvent).detail
    setStatus(detail?.message || "Processing...")
  }, [])

  const handleDone = useCallback((e: Event) => {
    const detail = (e as CustomEvent).detail
    setDone(true)
    setStatus(detail?.message || "Complete")
    setTimeout(() => setActive(false), 800)
  }, [])

  useEffect(() => {
    window.addEventListener("analysis:start", handleStart)
    window.addEventListener("analysis:progress", handleProgress)
    window.addEventListener("analysis:done", handleDone)
    return () => {
      window.removeEventListener("analysis:start", handleStart)
      window.removeEventListener("analysis:progress", handleProgress)
      window.removeEventListener("analysis:done", handleDone)
    }
  }, [handleStart, handleProgress, handleDone])

  const dots = useMemo(() => {
    const size = 8
    const gap = 4
    const step = size + gap
    const cols = Math.ceil(768 / step)
    const rows = Math.ceil(80 / step)
    const cells: GridDot[] = []
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        cells.push({
          id: `${runKey}-${r}-${c}`,
          x: c * step,
          y: r * step,
          delay: Math.random() * 1800,
          opacity: Math.random() * 0.6 + 0.3,
        })
      }
    }
    return cells
  }, [runKey])

  if (!active) return null

  return (
    <div className="mt-4 rounded-lg overflow-hidden">
      <div
        className="relative overflow-hidden mx-auto rounded-lg"
        style={{ width: '100%', height: 80, maxWidth: 768 }}
      >
        {dots.map(dot => (
          <div
            key={dot.id}
            className="absolute rounded-full"
            style={{
              left: dot.x,
              top: dot.y,
              width: 8,
              height: 8,
              backgroundColor: '#6366f1',
              opacity: dot.opacity,
              animation: done
                ? `analysis-fade 600ms ${dot.delay * 0.3}ms forwards`
                : `analysis-blink 1800ms ${dot.delay}ms infinite`,
            }}
          />
        ))}
      </div>
      <p className="mt-2 text-xs text-neutral-400 text-center animate-pulse">
        {status}
      </p>
    </div>
  )
}
