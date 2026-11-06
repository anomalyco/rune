import { StrictMode, startTransition } from "react"
import { createRoot, hydrateRoot } from "react-dom/client"
import { App } from "./App"
import { stripBase } from "./base"
import { attachReveals } from "./motion/reveal"
import "./system/tokens.css"
import "./site.css"

const root = document.getElementById("root")!
const page = (
  <StrictMode>
    <App notFound={stripBase(location.pathname) !== "/"} />
  </StrictMode>
)

// The prerendered page is already whole, so hydration can run as a transition: React yields every few ms instead of
// holding the main thread for the whole tree.
if (root.firstElementChild) startTransition(() => void hydrateRoot(root, page))
else createRoot(root).render(page)
attachReveals()
