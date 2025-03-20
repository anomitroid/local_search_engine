console.log("Querying /api/search");
fetch("/api/search", {
    method: 'POST',
    mode: 'cors',
    cache: 'no-cache',
    credentials: 'same-origin',
    headers: {
        'Content-Type': 'application/json'
    },
    redirect: 'follow',
    referrerPolicy: 'no-referrer',
    body: JSON.stringify('{"Hello": "World"}')
}).then(response => console.log(response));