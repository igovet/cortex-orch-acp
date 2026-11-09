import { describe, expect, it } from "vitest";
import { capture, coordinatorResolution, emptyWorkspace, machineDefaults, profileChanges, sessionsOf } from "../src";

describe("task default routes", () => {
  it("keeps the exact configured connection route instead of picking the first model", () => {
    const machine = emptyWorkspace().machines[0];
    machine.config.surfaces.user_defaults.coordinator = {engine:"claude-agent-acp",model:"opus",effort:"high",parameters:{}};
    machine.config.catalogue["claude-agent-acp"].models = [{id:"opus",routeId:"route:owner",label:"Opus",efforts:["high"],observed:true}];
    expect(machineDefaults(machine).settings.model).toBe("route:owner");
  });
  it("keeps an exact default route identity, including when its model has withdrawn", () => {
    const machine = emptyWorkspace().machines[0];
    machine.config.surfaces.user_defaults.coordinator = null;
    machine.config.surfaces.user_defaults.routeDefault = {routeId:"route:withdrawn",connectionId:"connection",model:"opus",parameters:{thought_level:"high"}};
    expect(machineDefaults(machine).settings.model).toBe("route:withdrawn");
  });
});

describe("profile changes in words", () => {
  const changed = () => {
    const machine = emptyWorkspace().machines[0], c = machine.config;
    c.catalogue["claude-agent-acp"].models = [{id:"opus",label:"Opus 5.5",efforts:["high"],observed:true}];
    c.surfaces.mcp_servers = [{name:"memory",command:"m",args:[],env:[],enabled:true,roles:null,engines:null,allowedTools:[],readOnlyTools:[],coordinatorTools:null,alwaysLoadTools:null,readPaths:[],writePaths:[],network:false,runtimeDirEnv:[],handTools:[],unsupportedKeys:[],problem:null}];
    machine.connections = [{id:"cn_ab12cd34ef56",engine:"claude",label:"Claude",note:"",sessions:2,state:"connected",checkedAt:null,detail:{connectionId:"cn_ab12cd34ef56",provider:"",authMode:"",keyKind:null,endpoint:null,launchCommand:"",credential:"",qualified:true,createdAt:"",updatedAt:""}}];
    c.profiles = [{id:"p",name:"p",description:"",updatedAt:null,sections:capture(c.surfaces,sessionsOf(machine))}];
    c.active = "p";
    return machine;
  };
  it("has nothing to say when nothing changed", () => expect(profileChanges(changed())).toEqual([]));
  it("names models, settings, servers and connections, and no key or id", () => {
    const machine = changed(), c = machine.config;
    c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
    c.surfaces.routing.engines["claude-agent-acp"].policy = {opus:{effortFloor:null,maxEffort:"high",effortLadder:null,alsoStandardFor:null,preferredFor:null,preferredWhen:null,requestNames:null}};
    c.surfaces.user_defaults.language = "ru";
    c.surfaces.user_defaults.ceiling = {...c.surfaces.user_defaults.ceiling,attempts:3};
    c.surfaces.mcp_servers[0].enabled = false;
    machine.connections[0].sessions = 4;
    const lines = profileChanges(machine).flatMap(x => x.lines);
    expect(lines).toEqual(["Opus 5.5 turned on", "Opus 5.5 max effort: High", "Language: Russian", "Limits: fallback attempts changed", "memory turned off", "Claude: 4 sessions at most, was 2"]);
  });
  it("still says something for a setting with no sentence of its own", () => {
    const machine = changed();
    machine.config.surfaces.routing.legacyTiers = ["x"];
    expect(profileChanges(machine)).toEqual([{section:"routing",lines:["Other settings changed"]}]);
  });
  it("reads an exact route by its model", () => {
    const machine = changed(), c = machine.config;
    c.surfaces.user_defaults.routeDefault = {routeId:"xr_6b08617e228a",connectionId:"cn_ab12cd34ef56",model:"opus",parameters:{}};
    expect(coordinatorResolution(machine)).toEqual({ok:true,route:"Claude · Opus 5.5, exact route"});
  });
});
