import {createRequire} from 'node:module';try{process.stdout.write(createRequire(import.meta.url).resolve(process.env.LUUNA_ATLAS_RECIPE_PACKAGE,{paths:[process.cwd()]}))}catch{process.exit(1)}
