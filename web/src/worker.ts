import init, { info, split } from './wasm/dxo_sraw_wasm.js'

export type Meta = { frames: number; width: number; height: number; bits: number }
export type Request = { id: number; file: File }
export type Response =
  | { id: number; ok: true; meta: Meta; dngs: Uint8Array[] }
  | { id: number; ok: false; error: string }

const ready = init()

async function handle({ id, file }: Request): Promise<Response> {
  try {
    await ready
    const bytes = new Uint8Array(await file.arrayBuffer())
    const i = info(bytes)
    const meta = { frames: i.frames, width: i.width, height: i.height, bits: i.bits }
    i.free()
    return { id, ok: true, meta, dngs: split(bytes) as Uint8Array[] }
  } catch (e) {
    return { id, ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

// One file at a time keeps peak memory at ~1 input + its outputs.
let queue = Promise.resolve()
self.onmessage = (e: MessageEvent<Request>) => {
  queue = queue.then(async () => {
    const res = await handle(e.data)
    self.postMessage(res, { transfer: res.ok ? res.dngs.map((d) => d.buffer) : [] })
  })
}
