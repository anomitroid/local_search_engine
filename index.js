async function search(prompt) {
    console.log("Searching for: " + prompt);
    const results = document.getElementById('results');
    results.innerHTML = '';
    const response = await fetch("/api/search", {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json'
        },
        body: prompt
    });
    const json = await response.json();
    console.log(json);
    results.innerHTML = '';
    for (let [path, rank] of json) {
        let item = document.createElement("span");
        item.appendChild(document.createTextNode(path));
        item.appendChild(document.createElement("br"));
        results.appendChild(item);
    }
}

let query = document.getElementById('query');
let currentSearch = Promise.resolve();

query.addEventListener('input', () => {
    currentSearch = currentSearch.then(() => search(query.value));
});