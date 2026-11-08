--!strict
-- Runs once in every VM before the sandbox freezes the globals. `task` arrives with spawn, defer,
-- delay and the host half of cancel; this adds wait and cancel, the signal types and the wrapper
-- that turns a host async function into one a task calls like a plain function.
local task, WAIT, PARK, JOB, forget = ...

local yield = coroutine.yield
local isyieldable = coroutine.isyieldable
local running = coroutine.running
local close = coroutine.close
local spawn = task.spawn

local function needTask(name: string)
	if not isyieldable() then
		error(`{name} can only be called from a task (task.spawn, a handler or a connection), not while rendering`, 3)
	end
end

function task.wait(seconds: number?): number
	needTask("task.wait")
	return yield(WAIT, seconds or 0)
end

function task.cancel(thread: thread)
	forget(thread)
	if coroutine.status(thread) ~= "running" then
		close(thread)
	end
end

-- Signals are userdata with Luau methods. A `Signal` (a host's) has Connect, Once and Wait; a
-- `ScriptSignal` (one a script fires) adds Fire and Version. A `Connection` has Connected and
-- Disconnect. A connection's callback runs as a task, so it may wait.
type State = { list: { any }, waiting: { thread }, version: number }
local states: { [any]: State } = setmetatable({}, { __mode = "k" }) :: any
type Conn = { fn: (...any) -> (), once: boolean, connected: boolean, signal: any }
local connections: { [any]: Conn } = setmetatable({}, { __mode = "k" }) :: any

local Connection = {}

function Connection.Disconnect(self)
	local c = connections[self]
	if not c.connected then
		return
	end
	c.connected = false
	local list = states[c.signal].list
	local i = table.find(list, self)
	if i then
		table.remove(list, i)
	end
end

local function connectionIndex(self, key)
	if key == "Connected" then
		return connections[self].connected
	end
	return Connection[key]
end

local function connect(self, fn, once: boolean)
	if type(fn) ~= "function" then
		error("Connect and Once need a function", 3)
	end
	local proxy = newproxy(true)
	local mt = getmetatable(proxy)
	mt.__index = connectionIndex
	mt.__type = "Connection"
	connections[proxy] = { fn = fn, once = once, connected = true, signal = self }
	table.insert(states[self].list, proxy)
	return proxy
end

local function fire(self, ...)
	local s = states[self]
	s.version += 1
	if #s.list > 0 then
		for _, proxy in table.clone(s.list) do
			local c = connections[proxy]
			if c.connected then
				if c.once then
					Connection.Disconnect(proxy)
				end
				spawn(c.fn, ...)
			end
		end
	end
	if #s.waiting > 0 then
		local waiting = s.waiting
		s.waiting = {}
		for _, thread in waiting do
			spawn(thread, ...)
		end
	end
end

local Signal = {}

function Signal.Connect(self, fn)
	return connect(self, fn, false)
end

function Signal.Once(self, fn)
	return connect(self, fn, true)
end

function Signal.Wait(self)
	needTask("Signal:Wait")
	table.insert(states[self].waiting, (running()))
	return yield(PARK)
end

local ScriptSignal = table.clone(Signal)

ScriptSignal.Fire = fire

-- How many times the signal fired: a cheap version for view:Memo.
function ScriptSignal.Version(self): number
	return states[self].version
end

local function new(methods, name: string)
	return function()
		local proxy = newproxy(true)
		local mt = getmetatable(proxy)
		mt.__index = methods
		mt.__type = name
		states[proxy] = { list = {}, waiting = {}, version = 0 }
		return proxy
	end
end

local function async(submit: (...any) -> number, name: string)
	return function(...)
		needTask(name)
		return yield(JOB, submit(...))
	end
end

return {
	signal = new(table.freeze(Signal), "Signal"),
	scriptSignal = new(table.freeze(ScriptSignal), "ScriptSignal"),
	fire = fire,
	async = async,
}
