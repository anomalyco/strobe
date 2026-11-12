// Prints what it read from its standard input, prefixed: a pipe's far side (shell.bench.ts).
const text = (await Bun.stdin.text()).trim()
console.log("stdin " + text)
