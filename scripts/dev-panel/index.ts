import { createRemoteControls, formatRust, type RemoteControls } from "@kitlangton/knobs"
import { mountPage } from "@kitlangton/knobs/panel"

let controls: RemoteControls | undefined
let shape = ""
const socket = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`)
const send = (message: object) => {
  if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message))
}

socket.addEventListener("message", event => {
  const message = JSON.parse(event.data)
  if (!message) return
  if (message.type === "error") {
    console.error("Rune dev:", message.message)
    return
  }
  if (message.type !== "state") return
  const next = JSON.stringify(message.descriptors)
  if (!controls) {
    controls = createRemoteControls("rune", message.descriptors, {
      values: message.values,
      storage: null,
      tunable: key => !key.startsWith("rune."),
      onSet: (key, value) => send({ type: "set", key, value }),
      onSetBatch: changes => send({ type: "setBatch", changes }),
      onAction: key => send({ type: "action", key }),
      onReset: target => { if (!target.key) send({ type: "reset", ...target }) },
    })
    mountPage(controls, document.body, {
      title: "Rune",
      // Header, group and row copies all emit `rune::tune!`; playback/scene controls are not app code.
      copyAs: (keys, group) => {
        const descriptors = controls!.descriptors.filter(d => !d.key.startsWith("rune.") && (!keys || keys.includes(d.key)))
        const values = controls!.get()
        // Like knobs' built-in "rust": a selection or group with nothing changed copies its values as they are.
        return formatRust(descriptors, values, group)
          || (keys || group ? formatRust(descriptors, values, group, { includeDefaults: true }) : "")
      },
    })
  } else if (next !== shape) {
    controls.update(message.descriptors, message.values)
  } else {
    controls.sync(message.values, { replace: true })
  }
  shape = next
  for (const [key, value] of Object.entries<number>(message.readouts ?? {})) controls.watch(key, value)
})
socket.addEventListener("close", () => { document.title = "Rune · disconnected" })
socket.addEventListener("open", () => { document.title = "Rune" })
