// harness.ts under a module that defines a `globalThis.Bun` of its own (a Bun-to-Node port's shim), for script.bench.ts:
// hosted on Node, the host must still say Node and collect garbage with Node's collector.
;(globalThis as any).Bun ??= { env: process.env }
export * from "./harness.ts"
