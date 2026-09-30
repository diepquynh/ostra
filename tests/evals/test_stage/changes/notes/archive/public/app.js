// The page: lists the notes, adds new ones, and archives them through the API.

const list = document.querySelector("#notes");
const form = document.querySelector("#new-note");
const showArchived = document.querySelector("#show-archived");

function renderNote(note) {
  const item = document.createElement("li");
  item.textContent = note.text;
  if (!note.archived) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = "Archive";
    button.addEventListener("click", async () => {
      await fetch(`/api/notes/${note.id}/archive`, { method: "POST" });
      await refresh();
    });
    item.append(" ", button);
  }
  return item;
}

async function refresh() {
  const response = await fetch(`/api/notes?archived=${showArchived.checked}`);
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

showArchived.addEventListener("change", refresh);

refresh();
