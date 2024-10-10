// MiniSamp.cpp : Ten plik zawiera funkcję „main”. W nim rozpoczyna się i kończy wykonywanie programu.
//

#include "RakPeerInterface.h"
#include "MessageIdentifiers.h"
#include "BitStream.h"
#include "RakNetTypes.h"
#include <iostream>
#include "../Common/MessageTypes.cpp"

void ProcessGameMessage(RakNet::Packet* packet) {
	RakNet::BitStream bsIn(packet->data, packet->length, false);
	bsIn.IgnoreBytes(sizeof(RakNet::MessageID));  // Ignore the message ID

	// Read the message
	char message[256];
	bsIn.Read(message);
	std::cout << "Received custom message from client: " << message << std::endl;
}

int main()
{
    std::cout << "Starting the server" << std::endl;

    RakNet::RakPeerInterface* peer = RakNet::RakPeerInterface::GetInstance();
    RakNet::SocketDescriptor sd(6000, 0);
    peer->SetMaximumIncomingConnections(2);

	if (peer->Startup(10, &sd, 1) != RakNet::RAKNET_STARTED) {
		std::cout << "Failed to start the server" << std::endl;
		return 1;
	} else {
		std::cout << "Server started, waiting for connections..." << std::endl;
	}

    while (true)
    {
		RakNet::Packet* packet = peer->Receive();
        if (packet)
        {
            switch (packet->data[0])
            {
			case ID_NEW_INCOMING_CONNECTION:
				std::cout << "New connection from " << packet->systemAddress.ToString() << std::endl;
				break;
			case ID_DISCONNECTION_NOTIFICATION:
				std::cout << "Disconnection from " << packet->systemAddress.ToString() << std::endl;
				break;
			case ID_CONNECTION_LOST:
				std::cout << "Lost connection from " << packet->systemAddress.ToString() << std::endl;
				break;
			case ID_CONNECTION_REQUEST_ACCEPTED:
				std::cout << "Connection request accepted" << std::endl;
				break;
			case ID_GAME_MESSAGE_1:
				ProcessGameMessage(packet);
				break;
			default:
				std::cout << "Message with identifier " << packet->data[0] << " received" << std::endl;
				break;
			}

			peer->DeallocatePacket(packet);
		}
	}

	peer->Shutdown(300);
	RakNet::RakPeerInterface::DestroyInstance(peer);

	return 0;
}

