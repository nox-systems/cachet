The workerd lane's stand-in for a built console.

`just workerd` proves the routing law, which is that the Rust router
decides every path and the asset layer only ever answers what the router
hands it. That law is about paths, so the lane needs a shell and one
hashed file and nothing else; building the real console here would put a
Vite build inside a lane that runs on every commit, for no assertion the
real bundle could make that these two files cannot.

The real bundle's serving is proven live instead, by the integration
lane against a deployment.
