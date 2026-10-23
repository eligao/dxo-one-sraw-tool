import { useEffect, useRef, useState } from 'react'
import { zipSync } from 'fflate'
import type { Meta, Request, Response } from './worker'

type Output = { name: string; blob: Blob; url: string }
type Item = {
  id: number
  name: string
  status: 'working' | 'done' | 'error'
  meta?: Meta
  outputs?: Output[]
  error?: string
}

const stem = (name: string) => name.replace(/\.[^.]+$/, '')
const mb = (n: number) => `${(n / 1e6).toFixed(1)} MB`

function download(blob: Blob, name: string) {
  const a = document.createElement('a')
  a.href = URL.createObjectURL(blob)
  a.download = name
  a.click()
  setTimeout(() => URL.revokeObjectURL(a.href), 10_000)
}

export default function App() {
  const [items, setItems] = useState<Item[]>([])
  const [dragging, setDragging] = useState(false)
  const worker = useRef<Worker>(null)
  const nextId = useRef(0)

  useEffect(() => {
    const w = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' })
    w.onmessage = ({ data: res }: MessageEvent<Response>) =>
      setItems((items) =>
        items.map((it) => {
          if (it.id !== res.id) return it
          if (!res.ok) return { ...it, status: 'error', error: res.error }
          const outputs = res.dngs.map((d, k) => {
            const blob = new Blob([d as Uint8Array<ArrayBuffer>], { type: 'image/x-adobe-dng' })
            return { name: `${stem(it.name)}_${k + 1}.dng`, blob, url: URL.createObjectURL(blob) }
          })
          return { ...it, status: 'done', meta: res.meta, outputs }
        }),
      )
    worker.current = w
    return () => w.terminate()
  }, [])

  function add(files: FileList | null) {
    for (const file of files ?? []) {
      const id = nextId.current++
      setItems((items) => [...items, { id, name: file.name, status: 'working' }])
      worker.current?.postMessage({ id, file } satisfies Request)
    }
  }

  function clear() {
    items.flatMap((it) => it.outputs ?? []).forEach((o) => URL.revokeObjectURL(o.url))
    setItems([])
  }

  async function downloadZip() {
    const entries: Record<string, [Uint8Array, { level: 0 }]> = {}
    for (const o of items.flatMap((it) => it.outputs ?? [])) {
      entries[o.name] = [new Uint8Array(await o.blob.arrayBuffer()), { level: 0 }]
    }
    download(new Blob([zipSync(entries) as Uint8Array<ArrayBuffer>], { type: 'application/zip' }), 'dxo-sraw.zip')
  }

  const doneCount = items.filter((it) => it.status === 'done').length

  return (
    <main>
      <header>
        <h1>DxO ONE SuperRAW splitter</h1>
        <a className="gh" href="https://github.com/eligao/dxo-one-sraw-tool" target="_blank" rel="noreferrer">
          ☆ GitHub
        </a>
      </header>
      <p>
        Split DxO ONE SuperRAW <code>.DXO</code> files into 4 standard DNGs, ready to merge in Lightroom, darktable,
        etc.<sup>*</sup>
      </p>
      <p>Everything runs in your browser; files are never uploaded.</p>

      <label
        className={`drop${dragging ? ' over' : ''}`}
        onDragOver={(e) => {
          e.preventDefault()
          setDragging(true)
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          e.preventDefault()
          setDragging(false)
          add(e.dataTransfer.files)
        }}
      >
        <input type="file" accept=".dxo,.DXO" multiple onChange={(e) => add(e.target.files)} />
        Drop <code>.DXO</code> files here, or click to choose
      </label>

      {items.length > 0 && (
        <>
          <div className="actions">
            <button onClick={downloadZip} disabled={doneCount === 0}>
              Download all as .zip
            </button>
            <button onClick={clear}>Clear</button>
          </div>
          <ul className="items">
            {items.map((it) => (
              <li key={it.id}>
                <strong>{it.name}</strong> {it.status === 'working' && <span className="muted">splitting…</span>}
                {it.status === 'error' && <span className="error">{it.error}</span>}
                {it.meta && (
                  <span className="muted">
                    {it.meta.frames} frames · {it.meta.width}×{it.meta.height} · {it.meta.bits}-bit
                  </span>
                )}
                {it.outputs && (
                  <div className="outputs">
                    {it.outputs.map((o) => (
                      <a key={o.name} href={o.url} download={o.name}>
                        {o.name} <span className="muted">({mb(o.blob.size)})</span>
                      </a>
                    ))}
                  </div>
                )}
              </li>
            ))}
          </ul>
        </>
      )}

      <footer className="muted">
        <p>* How it works:</p>
        <ul>
          <li>
            <a
              href="https://reference-dxoone.dxo.com/en/6-2-file-format-and-managing-raw-files/"
              target="_blank"
              rel="noreferrer"
            >
              SuperRAW
            </a>
            :{' '}
            <q>
              a DxO proprietary format, particularly suitable for high sensitivity shooting, combining four images in
              DNG format. It can be post-processed to merge the data contained in these four photos in order to obtain a
              single photo with less noise than any one of the original four photos
            </q>
            , but only with DxO's own software.
          </li>
          <li>
            Other raw software may open <code>.DXO</code> files, but only the first frame is read; the remaining 3 are
            ignored. Once split, all 4 raw frames can be merged properly, e.g. with Lightroom's Photo Merge → HDR or
            darktable's “create HDR”.
          </li>
        </ul>
      </footer>
    </main>
  )
}
