// Contrato da NUI do HexMP:
//   recebe:  window.addEventListener("message", e => e.data)   (o que o script mandou com ui.send)
//   chama:   hexmp.callback(nome, dados) -> Promise            (handler registrado com ui.on)
// `window.hexmp` é injetado pelo cliente do jogo.

const $ = (id) => document.getElementById(id);
let hideTimer;

function log(text) {
  const line = document.createElement("div");
  line.textContent = text;
  $("log").append(line);
  while ($("log").children.length > 5) $("log").firstChild.remove();
}

window.addEventListener("message", (e) => {
  const msg = e.data || {};
  if (msg.type === "announce") {
    $("announce").textContent = msg.text;
    $("announce").hidden = false;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => ($("announce").hidden = true), 8000);
  } else if (msg.type === "pong") {
    log(msg.text);
  } else if (msg.type === "ready") {
    log(`conectado como jogador ${msg.id}`);
  }
});

$("ping").addEventListener("submit", (e) => {
  e.preventDefault();
  const text = $("text").value.trim();
  if (!text || !window.hexmp) return;
  window.hexmp.callback("ping", { text });
  $("text").value = "";
});
