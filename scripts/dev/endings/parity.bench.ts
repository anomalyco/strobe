import { bench, web } from "@hona/strobe"
const side = (title: string) => web({headless:true, prepare: () => {
  const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>new Response(`<h1>${title}</h1>`,{headers:{"Content-Type":"text/html"}})})
  return {address:server.url.href}
},url:({prepared})=>prepared.address})
export default bench({
  targets: { left:side("before"),right:side("after") },
  cases: { journey: {primary:"count.items",expect:{same:["h1"]},run:({count})=>count("items",1)} },
})
