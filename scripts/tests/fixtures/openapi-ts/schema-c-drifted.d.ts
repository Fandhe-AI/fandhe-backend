export interface paths {
    "/health": {
        get: operations["health_doc"];
    };
    "/hello/{name}": {
        get: operations["hello_doc"];
    };
}
