import { bench, web } from "@hona/strobe"
const side = (title: string) => web({headless:true, url: () => `data:text/html,<h1>${title}</h1>`})
export default bench({
  targets: { left:side("before"),right:side("after") },
  cases: { journey: {primary:"count.items",expect:{same:["h1"]},run:({count})=>count("items",1)} },
})
