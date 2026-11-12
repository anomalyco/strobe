// Prints what it read from its standard input, prefixed: a pipe's far side (rows.ts). Runs on bun and node.
let text = ""
process.stdin.setEncoding("utf8")
process.stdin.on("data", (d) => (text += d))
process.stdin.on("end", () => console.log("stdin " + text.trim()))