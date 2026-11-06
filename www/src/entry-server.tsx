import { StrictMode } from "react"
import { renderToString } from "react-dom/server"
import { App } from "./App"

export function render(pathname: string): string {
  return renderToString(
    <StrictMode>
      <App notFound={pathname !== "/"} />
    </StrictMode>,
  )
}
