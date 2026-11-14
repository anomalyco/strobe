// Inspect what the uploader would write, without uploading anything.
import assert from 'node:assert/strict'
import {scrubResult} from '../../ci-action'

const screen = { display:'on', compositor:{name:'gnome-shell', cpu_pct:3.1} }
const block = {screen,background:[['busy-canary',42]]}
const result = {text:'screen  display on · session unlocked · GNOME-SHELL 3.1% of a core', screen,
  plan:{reserve:{moved:{'moved-canary':1},failed:{'failed-canary':'denied'}}},series:[{screen,blocks:[block],pass:{blocks:[block]}}]}
const copy = scrubResult(result)
for (const name of [...Object.keys(result.plan.reserve.moved),...Object.keys(result.plan.reserve.failed),block.background[0][0],screen.compositor.name]) {
  assert(!JSON.stringify(copy).toLowerCase().includes(String(name)))
}
for (const row of [copy,copy.series[0],copy.series[0].blocks[0],copy.series[0].pass.blocks[0]]) assert.deepEqual(row.screen.compositor,{cpu_pct:3.1})
assert(copy.text.endsWith('compositor 3.1% of a core'))
assert.equal(result.screen.compositor.name,'gnome-shell')
console.log('upload copy: screen roles, shares and counts retained; owner names absent at all four levels')
