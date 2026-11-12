// Prints the arguments it was given, one JSON line: what a command line passed (rows.ts). Runs on bun and node.
console.log("argv " + JSON.stringify(process.argv.slice(2)))