"use client"

import type React from "react"
import { useRef } from "react"
import { MeshGradient } from "@paper-design/shaders-react"

interface ShaderBackgroundProps {
  children: React.ReactNode
  className?: string
}

export function ShaderBackground({ children, className = "" }: ShaderBackgroundProps) {
  const containerRef = useRef<HTMLDivElement>(null)

  return (
    <div ref={containerRef} className={`w-full relative overflow-hidden ${className}`}>
      <svg className="absolute inset-0 w-0 h-0">
        <defs>
          <filter id="glass-effect" x="-50%" y="-50%" width="200%" height="200%">
            <feTurbulence baseFrequency="0.005" numOctaves="1" result="noise" />
            <feDisplacementMap in="SourceGraphic" in2="noise" scale="0.3" />
            <feColorMatrix
              type="matrix"
              values="1 0 0 0 0.02
                      0 1 0 0 0.02
                      0 0 1 0 0.05
                      0 0 0 0.9 0"
              result="tint"
            />
          </filter>
        </defs>
      </svg>

      <MeshGradient
        className="absolute inset-0 w-full h-full"
        colors={["#030014", "#4f46e5", "#0f172a", "#1e1b4b", "#7c3aed"]}
        speed={0.25}
        backgroundColor="#030014"
      />
      <MeshGradient
        className="absolute inset-0 w-full h-full opacity-30"
        colors={["#030014", "#818cf8", "#4f46e5", "#030014"]}
        speed={0.15}
        wireframe={true}
        backgroundColor="transparent"
      />

      <div className="absolute inset-0 bg-gradient-to-b from-transparent via-transparent to-[#0a0a0b]" />

      {children}
    </div>
  )
}
