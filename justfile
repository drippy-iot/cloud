# Start the PostgreSQL daemon.
db:
    @postgres -D data

# Create the `/data` folder.
initdb:
    @initdb -D data -U postgres

# Modify the template database so that it matches our initialization script.
template:
    @psql -U postgres -f db/init.sql -1 template1

# Instantiate the template database.
create:
    @createdb -U postgres drippy

# Drop the instantiated database.
drop:
    @dropdb -U postgres drippy

# Completely delete the entire `/data` folder.
nuke:
    @rm -r data

# Create the `/data` folder and start the database daemon in one step.
init: initdb && db

# Initialize and instantiate the template in one step.
instantiate: template && create

# Drop the instantiated database and recreate a new one from the template.
respawn: drop && create

# Nuke the entire `/data` folder and reinitialize it.
reset: nuke && init
