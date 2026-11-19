#!/bin/sh
# Forwards requests from the Elixir window to the Elixir service.
exec /var/packages/Elixir/target/bin/elixir cgi
