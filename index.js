console.log("Querying /api/search");
fetch("/api/search", {
    method: 'POST',
    headers: {
        'Content-Type': 'text/plain'
    },
    body: "glsl function for linear interpolation",
}).then(response => console.log(response));