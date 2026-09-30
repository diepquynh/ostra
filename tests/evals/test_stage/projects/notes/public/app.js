// The page: lists the notes and adds new ones through the API.

const list = document.querySelector("#notes");
const form = document.querySelector("#new-note");

function renderNote(note) {
  const item = document.createElement("li");
  item.textContent = note.text;
  return item;
}

async function refresh() {
  const response = await fetch("/api/notes");
  const notes = await response.json();
  list.replaceChildren(...notes.map(renderNote));
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const text = form.elements.text.value;
  await fetch("/api/notes", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });
  form.reset();
  await refresh();
});

refresh();
