#!/usr/bin/env bun
// Runs a worker-built example on this host, without a window or GPU allocation.
const binary = process.argv[2]
if (!binary) throw new Error("usage: bun scripts/dev-panel/smoke.ts /path/to/rune-dev-panel")
const processHandle = Bun.spawn([binary, "--windowless"], { stdout: "pipe", stderr: "pipe" })
let socket: WebSocket | undefined
async function run() {
  let text = ""
  let address: string | undefined
  for await (const bytes of processHandle.stderr) {
    text += new TextDecoder().decode(bytes)
    address = text.match(/Rune dev controls: (ws:\/\/\S+)/)?.[1]
    if (address) break
  }
  if (!address) throw new Error(`No dev socket announced: ${text}`)
  socket = new WebSocket(address)
  let nextID = 0
  const requests = new Map<number, (value: any) => void>()
  const ready = Promise.withResolvers<void>()
  socket.addEventListener("message", event => {
    const value = JSON.parse(event.data)
    if (value?.type === "state" && value.descriptors.length >= 4) ready.resolve()
    if (typeof value?.id === "number") {
      requests.get(value.id)?.(value)
      requests.delete(value.id)
    }
  })
  await ready.promise
  const request = (message: object) => new Promise<any>(resolve => {
    const id = ++nextID
    requests.set(id, resolve)
    socket!.send(JSON.stringify({ ...message, id }))
  })
  const ack = await request({ type: "set", key: "rune.state", value: "joining" })
  if (ack.type !== "ack") throw new Error(JSON.stringify(ack))
  const tree = await request({ type: "tree" })
  if (!tree.elements.some((e: any) => e.text === "Joining")) throw new Error("Scene did not reach the tree")
  const spring = await request({ type: "set", key: "rooms.join.spring", value: { duration: 0.55, bounce: 0 } })
  if (spring.type !== "ack") throw new Error(JSON.stringify(spring))
  console.log(JSON.stringify({ host: "local", socket: address, scene: "Joining", spring: 0.55, frame: spring.frame }))
}
let timeout: ReturnType<typeof setTimeout>
try {
  await Promise.race([run(), new Promise<never>((_, reject) => {
    timeout = setTimeout(() => reject(new Error("Rune dev smoke timed out")), 15_000)
  })])
} finally {
  clearTimeout(timeout!)
  socket?.close()
  processHandle.kill()
  await processHandle.exited
}
