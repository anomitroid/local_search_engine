async function search(prompt) {
    console.log(prompt);
    let results = document.getElementById('results');
    results.innerHTML = '';
    const response = await fetch("/api/search", {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json'
        },
        body: prompt
    });
    console.log(response);
    if (!response.ok) {
        results.innerHTML = 'Error: ' + response.status;
        return;
    }
    for ([path, rank] of await response.json()) {
        let item = document.createElement("span");
        item.appendChild(document.createTextNode(path));
        item.appendChild(document.createElement("br"));
        results.appendChild(item);
    }
}

let query = document.getElementById('query');
query.addEventListener('keypress', (e) => {
    if (e.key === 'Enter') {
        search(query.value);
    }
});