dbuser := "postgres"
dbname := "drippy"
dbdata := "data"

# Start the PostgreSQL daemon.
db:
    @postgres -D {{dbdata}}

shell:
    @psql -U {{dbuser}} {{dbname}}

# Create the `/data` folder.
initdb:
    @initdb -D {{dbdata}} -U {{dbuser}}

# Modify the template database so that it matches our initialization script.
template:
    @psql -U {{dbuser}} -f db/init.sql -1 template1

# Instantiate the template database.
create:
    @createdb -U {{dbuser}} {{dbname}}

# Drop the instantiated database.
drop:
    @dropdb -U {{dbuser}} {{dbname}}

# Completely delete the entire `/data` folder.
nuke:
    @rm -r {{dbdata}}

# Create the `/data` folder and start the database daemon in one step.
init: initdb && db

# Initialize and instantiate the template in one step.
instantiate: template && create

# Drop the instantiated database and recreate a new one from the template.
respawn: drop && create

# Nuke the entire `/data` folder and reinitialize it.
reset: nuke && init
