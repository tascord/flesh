import { useEffect, useState } from "react";
import { useChat, type Message } from "./api";
import { useDebounce } from "use-debounce";

export default function Chat() {
    return (
        <div className="w-screen h-screen flex flex-col text-white bg-black font-mono p-4 overflow-clip">
            <nav className="mb-2">
                <h1 className="font-bold text-xl">FLESH::Chat</h1>
            </nav>
            <div className="flex w-full h-full">
                <Channels />
                <Messages />
            </div>
        </div>
    )
}

function Channels() {
    const { channels: list, change_channel } = useChat();

    return (
        <div className="w-[20rem]">
            <h2 className="font-semibold text-lg mb-1">Channels</h2>
            <ul>
                {list.map(l => <ul><a href="#" onClick={(e) => {
                    e.preventDefault();
                    change_channel(l);
                }}>{l}</a></ul>)}
            </ul>
        </div>
    )
}

const colours = [
    '#ff0000',
    '#00ff00',
    '#ffff00',
    '#0000ff',
    '#ff00ff',
    '#00ffff',
];

function Messages() {
    const { messages, current_server, current_channel_name, join, send } = useChat();
    const [message, setMessage] = useState<string>();
    const [tmpUser, setTmpUser] = useState('anonymous');
    const [value] = useDebounce(tmpUser, 500);
    useEffect(() => {
        join(value.replace(/[^A-z0-9_]/g, '').trim())
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [value]);

    const colour = (v: string) => <span style={{ color: colours.at(v.split('').map(c => c.charCodeAt(0)).reduce((a, b) => a + b, 0) % colours.length)! }}>{v}</span>;
    const to_dom = (m: Message) => {
        if ('Join' in m) {
            return <span className="opacity-60 italic">{colour(m.Join)} Joined</span>
        } else if ('Text' in m) {
            return <span>{colour(m.Text.author)}: {m.Text.content}</span>
        }
    }

    return (
        <div className="flex flex-col h-full w-full">
            <h2 className="font-semibold text-lg mb-1">#{current_channel_name} <span className="text-xs">via {current_server}</span></h2>
            <div className="flex flex-col flex-1 h-full pb-4 max-h-full overflow-x-auto">
                {messages.filter(m => {
                    if ('Text' in m && m.Text.channel !== current_channel_name) return false;
                    return true;
                }).map(to_dom)}
            </div>
            <div className="h-[10rem] pt-4">
                <div className="relative text-xs">
                    <input type="text" maxLength={20} className="field-sizing-content" style={{ minInlineSize: '10ch', maxInlineSize: '20ch' }} placeholder="anonymous" value={tmpUser} onChange={v => {
                        setTmpUser(v.target.value.replace(/[^A-z0-9_]g/, ''));
                    }} />
                    <span>@{current_server}</span>
                </div>
                <form className="flex pt-2 w-full items-center justify-center" onSubmit={e => {
                    e.preventDefault();
                    if ((message?.trim().length || 0) > 0) {
                        send(String(message));
                        setMessage('');
                    }
                }}>
                    <textarea className="w-full resize-none" placeholder={`Message #${current_channel_name}`} value={message} onChange={e => setMessage(e.target.value.slice(0, 250).replace(/ +/g, ' '))} onKeyDown={e => {
                        if (e.key == 'Enter' && !e.shiftKey) {
                            if ((message?.trim().length || 0) > 0) {
                                send(String(message));
                                setMessage('');
                            }
                        }
                    }} />
                    <button type="submit" className="text-2xl border-white border w-12 h-12 grid place-items-center">
                        <span className="block">
                            →
                        </span>
                    </button>
                </form>
            </div>
        </div>
    )
}