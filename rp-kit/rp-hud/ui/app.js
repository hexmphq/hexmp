const $ = (id) => document.getElementById(id);
const call = (name, data) => window.hexmp && window.hexmp.callback(name, data);
const money = (v) => "$ " + Number(v).toLocaleString("pt-BR");
const list = (v) => (Array.isArray(v) ? v : []); // tabela vazia de Luau chega como objeto
let bag = null;
let others = [];
let giving = null;
let diffTimer;

function el(tag, text, cls) {
  const e = document.createElement(tag);
  if (text !== undefined) e.textContent = text;
  if (cls) e.className = cls;
  return e;
}

function button(text, run) {
  const b = el("button", text);
  b.onclick = run;
  return b;
}

function drawBag() {
  if (!bag) return;
  const kg = (g) => (g / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 });
  $("weight").textContent = `${kg(bag.weight)} / ${kg(bag.maxWeight)} kg`;
  $("bar").firstElementChild.style.width = `${Math.min(100, (100 * bag.weight) / bag.maxWeight)}%`;
  $("items").replaceChildren();
  for (const i of list(bag.items)) {
    const li = el("li");
    const name = el("span", i.label, "name");
    name.prepend(el("b", `${i.amount}x`));
    li.append(name);
    if (i.usable) li.append(button("Usar", () => call("use", { item: i.item })));
    if (others.length) {
      li.append(button("Dar", () => {
        giving = giving === i.item ? null : i.item;
        drawBag();
      }));
    }
    if (giving === i.item) {
      const to = el("div", undefined, "to");
      for (const p of others) {
        to.append(button(p.name, () => {
          giving = null;
          drawBag();
          call("give", { item: i.item, player: p.id });
        }));
      }
      li.append(to);
    }
    $("items").append(li);
  }
  if (!list(bag.items).length) $("items").append(el("li", "Nada aqui ainda."));
}

function notify(text, kind) {
  const n = el("div", text, kind);
  $("notes").append(n);
  while ($("notes").children.length > 4) $("notes").firstChild.remove();
  setTimeout(() => n.remove(), 5000);
}

function drawPoints(points) {
  $("points").replaceChildren();
  for (const p of list(points)) {
    const li = el("li");
    li.innerHTML = '<svg viewBox="0 0 14 14"><path d="M7 1l5 12-5-3-5 3z" fill="#40daff"/></svg>';
    li.firstChild.style.transform = `rotate(${p.heading}deg)`;
    li.append(el("span", p.label), el("em", `${p.distance} m`));
    $("points").append(li);
  }
}

function drawNear(players) {
  const ul = $("near").querySelector("ul");
  ul.replaceChildren();
  for (const p of list(players)) {
    const li = el("li");
    li.append(el("span", p.name), el("em", `${p.distance} m`));
    ul.append(li);
  }
  $("near").hidden = !ul.children.length;
}

window.addEventListener("message", (e) => {
  const m = e.data || {};
  if (m.type === "state") {
    $("hud").hidden = !m.playing || !m.money;
    if (m.money) {
      $("cash").textContent = money(m.money.cash);
      $("bank").textContent = money(m.money.bank);
    }
    bag = m.bag || null;
    drawBag();
  } else if (m.type === "cash") {
    $("diff").textContent = (m.diff > 0 ? "+" : "\u2212") + money(Math.abs(m.diff));
    $("diff").className = m.diff > 0 ? "up" : "down";
    clearTimeout(diffTimer);
    diffTimer = setTimeout(() => ($("diff").textContent = ""), 3000);
  } else if (m.type === "prompt") {
    $("prompt").textContent = m.text;
    $("prompt").hidden = !m.text;
  } else if (m.type === "points") drawPoints(m.points);
  else if (m.type === "near") drawNear(m.players);
  else if (m.type === "notify") notify(m.text, m.kind);
  else if (m.type === "inventory") {
    others = list(m.players).filter((p) => p.id !== m.me);
    giving = null;
    $("inventory").hidden = !m.open;
    drawBag();
  }
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("inventory").hidden) call("close");
});

call("ready");
