const $ = (id) => document.getElementById(id);
const call = (name, data) => window.hexmp && window.hexmp.callback(name, data);
const KEEP = 30; // mensagens guardadas
const SHOW_MS = 10000; // depois disso somem (voltam ao abrir o chat)

function add(m) {
  const li = document.createElement("li");
  li.className = m.kind;
  if (m.kind === "me") {
    li.textContent = `* ${m.from} ${m.text}`;
  } else {
    if (m.from) {
      const b = document.createElement("b");
      b.textContent = m.from + (m.kind === "say" ? ":" : "");
      li.append(b);
    }
    li.append(document.createTextNode(m.text));
  }
  $("log").append(li);
  while ($("log").children.length > KEEP) $("log").firstChild.remove();
  setTimeout(() => li.classList.add("old"), SHOW_MS);
}

function setOpen(on) {
  $("form").hidden = !on;
  $("chat").classList.toggle("open", on);
  if (on) {
    $("text").value = "";
    $("text").focus();
  }
}

$("form").addEventListener("submit", (e) => {
  e.preventDefault();
  const text = $("text").value.trim();
  if (text) call("send", { text });
  else call("close");
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("form").hidden) call("close");
});

window.addEventListener("message", (e) => {
  const m = e.data || {};
  if (m.type === "msg") add(m.msg || {});
  else if (m.type === "input") setOpen(!!m.open);
});
