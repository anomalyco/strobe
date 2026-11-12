// Prints FOO from its environment (rows.ts: a variable set in front of a command). Runs on bun and node.
console.log("env FOO=" + (process.env.FOO ?? ""))