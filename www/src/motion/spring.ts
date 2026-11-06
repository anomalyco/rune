// Rune's spring (crates/rune-motion/src/spring.rs and layer.rs) in TypeScript: described by perceptual duration and
// bounce like SwiftUI, solved in closed form, so any time samples exactly and a retarget can start from the current
// position and velocity.

export type Spring = { duration: number; bounce: number }

export const spring = (duration: number, bounce = 0): Spring => ({ duration, bounce })

export const omega = (s: Spring) => (2 * Math.PI) / s.duration
export const zeta = (s: Spring) => (s.bounce >= 0 ? 1 - s.bounce : 1 / (1 + s.bounce))

/** Offset from rest and velocity `t` seconds after starting at offset `a` with velocity `v0`. */
export function solve(s: Spring, a: number, v0: number, t: number): [number, number] {
  const w = omega(s), z = zeta(s)
  if (Math.abs(z - 1) < 1e-4) {
    const b = v0 + w * a, e = Math.exp(-w * t)
    return [(a + b * t) * e, (b - w * (a + b * t)) * e]
  }
  if (z < 1) {
    const sigma = z * w, wd = w * Math.sqrt(1 - z * z)
    const b = (v0 + sigma * a) / wd
    const e = Math.exp(-sigma * t), c = Math.cos(wd * t), sn = Math.sin(wd * t)
    const x = e * (a * c + b * sn)
    const v = e * ((b * wd - sigma * a) * c - (a * wd + sigma * b) * sn)
    return [x, v]
  }
  const root = Math.sqrt(z * z - 1)
  const r1 = -w * (z - root), r2 = -w * (z + root)
  const c2 = (v0 - r1 * a) / (r2 - r1), c1 = a - c2
  const e1 = Math.exp(r1 * t), e2 = Math.exp(r2 * t)
  return [c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2]
}

/** A value that springs to its target. `to` keeps the current position and, unless `keepVelocity` is off, velocity. */
export class Animated {
  private from = 0
  private v0 = 0
  private t0 = 0
  target: number

  constructor(value: number, public spring: Spring) {
    this.target = value
  }

  sample(now: number): [number, number] {
    const [x, v] = solve(this.spring, this.from - this.target, this.v0, Math.max(0, now - this.t0))
    return [this.target + x, v]
  }

  to(target: number, now: number, keepVelocity = true) {
    const [x, v] = this.sample(now)
    this.from = x
    this.v0 = keepVelocity ? v : 0
    this.t0 = now
    this.target = target
  }

  /** Jump: no motion, no velocity, as a finger moving the value directly. */
  set(value: number, now: number) {
    this.from = value
    this.target = value
    this.v0 = 0
    this.t0 = now
  }

  /** Let go: spring from where it is to `target`, starting at the finger's `velocity`. */
  release(target: number, velocity: number, now: number) {
    this.from = this.sample(now)[0]
    this.v0 = velocity
    this.t0 = now
    this.target = target
  }

  settled(now: number, eps = 0.05) {
    const [x, v] = this.sample(now)
    return Math.abs(x - this.target) < eps && Math.abs(v) < eps * 10
  }
}
