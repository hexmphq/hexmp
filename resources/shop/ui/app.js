const $ = (id) => document.getElementById(id);
const call = (name, data) => window.hexmp && window.hexmp.callback(name, data);

window.addEventListener("message", (e) => {
  const m = e.data || {};
  if (m.type !== "show") return;
  $("shop").hidden = !m.open;
  $("items").replaceChildren();
  for (const [id, item] of Object.entries(m.items).sort((a, b) => a[1].preco - b[1].preco)) {
    const li = document.createElement("li");
    for (const [tag, text] of [["span", item.nome], ["em", `$ ${item.preco.toLocaleString("pt-BR")}`], ["button", "Comprar"]]) {
      const part = document.createElement(tag);
      part.textContent = text;
      li.append(part);
    }
    li.lastChild.onclick = () => call("buy", { item: id });
    $("items").append(li);
  }
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") call("close");
});
