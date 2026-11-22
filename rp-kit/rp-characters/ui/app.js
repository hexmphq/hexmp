const $ = (id) => document.getElementById(id);
const call = (name, data) => window.hexmp && window.hexmp.callback(name, data);
let busy = false;

function form(on) {
  $("create").hidden = !on;
  $("choose").hidden = on;
  $("error").textContent = "";
  if (on) $("first").focus();
}

function add(el, tag, text) {
  const child = document.createElement(tag);
  child.textContent = text;
  el.append(child);
}

function show(msg) {
  busy = false;
  form(false);
  $("screen").hidden = false;
  $("error").textContent = msg.error || "";
  // Uma tabela vazia de Luau chega como objeto.
  const all = Array.isArray(msg.characters) ? msg.characters : [];
  $("list").replaceChildren();
  for (const c of all) {
    const b = document.createElement("button");
    add(b, "strong", `${c.firstName} ${c.lastName}`);
    add(b, "small", c.birthdate.split("-").reverse().join("/"));
    b.onclick = () => {
      if (busy) return;
      busy = true;
      call("select", { id: c.id });
    };
    $("list").append(b);
  }
  if (all.length < 3) {
    const b = document.createElement("button");
    b.className = "new";
    b.textContent = "+ Novo personagem";
    b.onclick = () => form(true);
    $("list").append(b);
  }
  $("list").firstChild.focus();
}

window.addEventListener("message", (e) => {
  const msg = e.data || {};
  if (msg.type === "list") show(msg);
  else if (msg.type === "hide") $("screen").hidden = true;
  else if (msg.type === "error") {
    busy = false;
    $("error").textContent = msg.text;
  }
});

$("back").onclick = () => form(false);
$("create").addEventListener("submit", (e) => {
  e.preventDefault();
  if (busy) return;
  busy = true;
  $("error").textContent = "";
  const two = (id) => $(id).value.trim().padStart(2, "0");
  call("create", {
    firstName: $("first").value.trim(),
    lastName: $("last").value.trim(),
    birthdate: `${$("year").value.trim()}-${two("month")}-${two("day")}`,
  });
});

call("ready");
