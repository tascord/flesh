/* eslint-disable @typescript-eslint/no-unused-vars */
import {
    createContext,
    useContext,
    useEffect,
    useMemo,
    useRef,
    useState,
    type ReactNode,
} from "react";

const RECONNECT_DELAY = 5000; // 5 seconds
const HEARTBEAT_INTERVAL = 5000; // 5 seconds

const default_context = {
    name: "anonymous" as string,
    channels: [] as string[],
    current_server: "localnode" as string,
    current_channel_name: "<waiting>" as string,
    messages: [] as Message[],
    send: (_: string) => { },
    join: (_: string) => { },
    change_channel: (_: string) => { },
};

const demo_context: Chat = {
    ...default_context,
    messages: [],
};

export type Message =
    | { Text: { author: string; content: string; channel: string } }
    | { Join: string }
    | { Channels: string[] }
    | { CurrentServer: string };
type Chat = typeof default_context;
const ChatContext = createContext<Chat>(default_context);

// eslint-disable-next-line react-refresh/only-export-components
export const useChat = () => useContext(ChatContext);

function connectWebSocket(wsRef: React.MutableRefObject<WebSocket | null>,
    onMessage: (m: MessageEvent) => void,
    onOpen: () => void,
    onClose: () => void) {
    if (wsRef.current && (wsRef.current.readyState === WebSocket.OPEN || wsRef.current.readyState === WebSocket.CONNECTING)) {
        return; // Already open or connecting
    }

    wsRef.current = new WebSocket("ws://localhost:8080");

    wsRef.current.onopen = onOpen;
    wsRef.current.onmessage = onMessage;
    wsRef.current.onclose = onClose;
    wsRef.current.onerror = (error) => console.error("WebSocket Error:", error);
}

export function ChatProvider({
    children,
}: {
    children: ReactNode | ReactNode[];
}) {
    const ws = useRef<WebSocket | null>(null);
    const reconnectTimerRef = useRef<number | null>(null);

    const [isConnected, setIsConnected] = useState(false);
    const [reconnectAttempt, setReconnectAttempt] = useState(0);
    const [chatData, setChatData] = useState<Omit<Chat, "send" | "join" | "change_channel">>({
        ...default_context,
        messages: demo_context.messages,
        channels: demo_context.channels,
        current_server: demo_context.current_server,
        current_channel_name: demo_context.current_channel_name,
    });

    const handleWsMessage = (m: MessageEvent) => {
        let js: Message;

        try {
            js = JSON.parse(m.data);
        } catch {
            return;
        }

        console.log(js);
        setChatData((prev_v) => {
            let updated_data = prev_v;
            // ... (Your existing immutable update logic) ...
            if ("Join" in js || "Text" in js) {
                updated_data = {
                    ...prev_v,
                    messages: [...prev_v.messages, js],
                };
            } else if ("Channels" in js) {
                let new_channel_name = prev_v.current_channel_name;
                if (
                    prev_v.current_channel_name === default_context.current_channel_name
                ) {
                    new_channel_name = js.Channels[0];
                }

                updated_data = {
                    ...prev_v,
                    channels: js.Channels,
                    current_channel_name: new_channel_name,
                };
            } else if ("CurrentServer" in js) {
                updated_data = {
                    ...prev_v,
                    current_server: js.CurrentServer,
                };
            }
            return updated_data;
        });
    };

    useEffect(() => {

        if (reconnectTimerRef.current !== null) {
            clearTimeout(reconnectTimerRef.current);
            reconnectTimerRef.current = null;
        }

        const onOpen = () => {
            console.log("WebSocket connected.");
            setIsConnected(true);
            setReconnectAttempt(0);
        };

        const onClose = () => {
            // console.log("WebSocket disconnected. Attempting reconnect...");
            // setIsConnected(false);

            // const timerId = window.setTimeout(() => {
            //     setReconnectAttempt(a => a + 1);
            // }, RECONNECT_DELAY);

            // reconnectTimerRef.current = timerId;
        };

        connectWebSocket(ws, handleWsMessage, onOpen, onClose);
        const heartbeat = window.setInterval(() => {
            if (ws.current?.readyState === WebSocket.CLOSED) {
                onClose();
            }
        }, HEARTBEAT_INTERVAL);


        return () => {
            clearInterval(heartbeat);
            if (reconnectTimerRef.current !== null) {
                clearTimeout(reconnectTimerRef.current);
                reconnectTimerRef.current = null;
            }

            if (ws.current) {
                ws.current.close(1000, "Component Unmount");
                ws.current = null;
            }
        };
    }, [reconnectAttempt]);

    const contextValue = useMemo(
        () => ({
            ...chatData,
            send: (t: string) => {
                if (ws.current?.readyState === WebSocket.OPEN && isConnected) {
                    ws.current.send(
                        JSON.stringify({
                            Text: {
                                author: chatData.name,
                                channel: chatData.current_channel_name,
                                content: t,
                            },
                        } as Message)
                    );
                } else {
                    console.warn("Cannot send message: WebSocket is not open.");
                }
            },
            join: (n: string) => {
                if (ws.current?.readyState === WebSocket.OPEN && isConnected) {
                    setChatData(cd => ({...cd, name: n}));
                    ws.current.send(
                        JSON.stringify({
                            Join: `${n}@${chatData.current_server}`,
                        } as Message)
                    );
                }
            },
            change_channel: (c: string) => {
                setChatData(cd => ({ ...cd, current_channel_name: c }));
            }
        }),
        [chatData, isConnected]
    );

    return (
        <ChatContext.Provider value={contextValue}>{children}</ChatContext.Provider>
    );
}
