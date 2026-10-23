import { BASE_PATH, stripBase, withBase } from "./src/base"

type Env = {
  ASSETS: { fetch(request: Request): Promise<Response> }
  CANONICAL_HOST: string
  ANY_HOST?: string
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url)

    if (!env.ANY_HOST && (url.hostname !== env.CANONICAL_HOST || url.protocol !== "https:")) {
      url.protocol = "https:"
      url.hostname = env.CANONICAL_HOST
      url.port = ""
      return Response.redirect(url.toString(), 301)
    }

    if (url.pathname === BASE_PATH) {
      url.pathname = `${BASE_PATH}/`
      return Response.redirect(url.toString(), 301)
    }

    // The route also matches paths like /team/runes, which are not ours.
    const path = stripBase(url.pathname)
    if (path === undefined) return new Response("Not found", { status: 404 })

    const assetUrl = new URL(url)
    assetUrl.pathname = path
    let response = await env.ASSETS.fetch(new Request(assetUrl, request))
    if (response.status === 404) {
      const page = await env.ASSETS.fetch(new Request(new URL("/404", assetUrl)))
      if (page.ok) response = new Response(page.body, { status: 404, headers: page.headers })
    }
    const headers = new Headers(response.headers)
    // Asset redirects (trailing slashes, /index.html) point at root-relative paths.
    const location = headers.get("Location")
    if (location?.startsWith("/")) headers.set("Location", withBase(location))
    if (path.startsWith("/assets/")) {
      headers.set("Cache-Control", "public, max-age=31536000, immutable")
    }
    headers.set("X-Content-Type-Options", "nosniff")
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    })
  },
}
