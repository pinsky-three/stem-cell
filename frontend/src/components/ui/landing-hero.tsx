"use client";

import { ShaderBackground } from "./hero-shader";
import {
  Shield,
  ClipboardCheck,
  Eye,
  Package,
  FileText,
  ArrowRight,
} from "lucide-react";

export default function LandingHero() {
  return (
    <ShaderBackground className="h-screen">
      {/* Full-height flex container — pushes content to the bottom */}
      <div className="absolute inset-0 z-20 flex flex-col justify-end px-8 sm:px-12 lg:px-16 pb-14 sm:pb-16">
        <div className="max-w-2xl">
          {/* Urgency badge */}
          <div
            className="inline-flex items-center gap-2 rounded-full bg-white/6 backdrop-blur-sm px-4 py-1.5 mb-6 border border-white/8"
            style={{ filter: "url(#glass-effect)" }}
          >
            <div className="absolute top-0 left-2 right-2 h-px bg-linear-to-r from-transparent via-white/15 to-transparent rounded-full" />
            <span className="relative flex h-2 w-2">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-amber-400 opacity-75" />
              <span className="relative inline-flex h-2 w-2 rounded-full bg-amber-400" />
            </span>
            <span className="text-white/80 text-xs font-medium">
              DS 115-2025-PCM deadline approaching
            </span>
          </div>

          {/* Headline */}
          <h1 className="text-5xl sm:text-6xl lg:text-7xl tracking-tight text-white leading-[1.05]">
            Map your
            <br />
            company's{" "}
            <span className="bg-linear-to-r from-indigo-300 via-violet-300 to-purple-300 bg-clip-text text-transparent italic font-light">
              AI risk
            </span>
          </h1>

          {/* Subheadline */}
          <p className="mt-5 max-w-lg text-sm sm:text-base text-white/50 font-light leading-relaxed">
            framework, and show you exactly what to fix next.
          </p>

          {/* CTAs */}
          <div className="mt-8 flex flex-wrap items-center gap-4">
            <a
              href="/audit"
              className="group flex items-center gap-2.5 rounded-full bg-white px-8 py-3 text-sm font-semibold text-neutral-900 transition-all duration-300 hover:bg-white/90 hover:shadow-lg hover:shadow-indigo-500/20 hover:scale-[1.02]"
            >
              Run free AI audit
              <ArrowRight className="h-4 w-4 transition-transform duration-300 group-hover:translate-x-0.5" />
            </a>
            <a
              href="#contact"
              className="flex items-center gap-2 rounded-full border border-white/15 px-8 py-3 text-sm font-medium text-white/70 transition-all duration-300 hover:bg-white/6 hover:border-white/25 hover:text-white"
            >
              Talk to an advisor
            </a>
          </div>
        </div>

        {/* Trust strip at the very bottom */}
        <div className="mt-10 flex flex-wrap items-center gap-x-6 gap-y-2 text-[11px] font-medium text-white/30">
          <span className="flex items-center gap-1.5">
            <Shield className="h-3 w-3 text-indigo-400/60" />
          </span>
          <span className="flex items-center gap-1.5">
            <ClipboardCheck className="h-3 w-3 text-indigo-400/60" />
            DS 115-2025-PCM aligned
          </span>
          <span className="flex items-center gap-1.5">
            <Eye className="h-3 w-3 text-indigo-400/60" />
            Human-reviewed outputs
          </span>
          <span className="flex items-center gap-1.5">
            <Package className="h-3 w-3 text-indigo-400/60" />
            Policy + technical remediation
          </span>
          <span className="flex items-center gap-1.5">
            <FileText className="h-3 w-3 text-indigo-400/60" />
            Audit-ready evidence pack
          </span>
        </div>
      </div>
    </ShaderBackground>
  );
}
