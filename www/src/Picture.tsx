import images from "./images.json"
import { withBase } from "./base"

type Name = keyof typeof images

/** One of Rune's renders from public/img (scripts/images.ts), as AVIF with a WebP fallback, sized up front so nothing
 *  shifts while it loads. `sizes` says how wide it shows. */
export function Picture({ name, alt, sizes, eager = false }: { name: Name; alt: string; sizes: string; eager?: boolean }) {
  const { width, height, widths } = images[name]
  const set = (ext: string) => widths.map(w => `${withBase(`/img/${name}-${w}.${ext}`)} ${w}w`).join(", ")
  return (
    <picture>
      <source type="image/avif" srcSet={set("avif")} sizes={sizes} />
      <img
        src={withBase(`/img/${name}-${widths.at(-1)}.webp`)}
        srcSet={set("webp")}
        sizes={sizes}
        width={width}
        height={height}
        alt={alt}
        loading={eager ? "eager" : "lazy"}
        decoding="async"
      />
    </picture>
  )
}
