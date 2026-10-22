// Browser input contract tests, using Node's built-in runner (no npm packages).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const vm = require('node:vm');

function player() {
  const handlers = {}, sent = [];
  const elements = {};
  for (const name of ['stage', 'connect', 'flag', 'stop', 'status', 'question', 'question-label', 'answer']) {
    elements[name] = {
      focus() {}, setPointerCapture() {},
      getBoundingClientRect: () => ({left: 100, top: 50, width: 960, height: 720}),
      addEventListener: (type, handler) => {handlers[`${name}:${type}`] = handler;}
    };
  }
  const context = vm.createContext({
    document: {
      querySelector: name => elements[name.slice(1)],
      addEventListener: (type, handler) => {handlers[`document:${type}`] = handler;}
    },
    window: {addEventListener: (type, handler) => {handlers[`window:${type}`] = handler;}},
    WebSocket: {OPEN: 1},
  });
  const html = readFileSync(`${__dirname}/../src/streaming/player.html`, 'utf8');
  vm.runInContext(html.split('<script>')[1].split('</script>')[0], context);
  context.record = data => sent.push(JSON.parse(data));
  vm.runInContext('socket = {readyState: 1, send: record};', context);
  return {handlers, elements, sent, context};
}

test('questions use plain text and submit Unicode answers with the active id', () => {
  const {elements,sent,context} = player();
  vm.runInContext('showQuestion({id:7,text:"<b>Name?</b>"})',context);
  assert.equal(elements['question-label'].textContent,'<b>Name?</b>');
  assert.equal(elements.question.hidden,false);
  elements.answer.value='Player 星';
  elements.question.onsubmit({preventDefault(){}});
  assert.deepEqual(sent.at(-1).event,{type:'answer',id:7,text:'Player 星'});
  assert.equal(elements.question.hidden,true);
});

test('focused stage sends normalized keys and prevents page scrolling', () => {
  const {handlers, sent} = player();
  let prevented = 0;
  const key = {key: 'ArrowRight', repeat: false, preventDefault() {prevented++;}};
  handlers['stage:keydown'](key);
  handlers['stage:keydown']({...key, repeat: true});
  handlers['stage:keyup'](key);
  assert.equal(prevented, 3);
  assert.deepEqual(sent.map(x => x.event), [
    {type: 'key', key: 'right arrow', down: true},
    {type: 'key', key: 'right arrow', down: false},
  ]);
});

test('scaled pointer coordinates and release use Scratch stage coordinates', () => {
  const {handlers, sent} = player();
  const point = {clientX: 820, clientY: 590, button: 0, isPrimary: true, pointerId: 1, preventDefault() {}};
  handlers['stage:pointerdown'](point);
  handlers['stage:pointerup'](point);
  assert.deepEqual(sent.map(x => x.event), [
    {type: 'mouse', x: 120, y: -90, down: true},
    {type: 'mouse', x: 120, y: -90, down: false},
  ]);
  handlers['stage:pointermove']({...point, clientX: 2000});
  assert.deepEqual(sent.at(-1).event, {type: 'mouse', x: 240, y: -90, down: false});
});

test('focus loss and player controls release held inputs', () => {
  const {handlers, elements, sent} = player();
  handlers['window:blur']();
  elements.flag.onclick();
  elements.stop.onclick();
  assert.deepEqual(sent.map(x => x.event.type), ['release', 'release', 'flag', 'release', 'stop']);
});
