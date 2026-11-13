// A child that lives about 30 ms: what a background job in a command line leaves running at the line's exit.
console.log("ok")
setTimeout(() => {}, 30)
